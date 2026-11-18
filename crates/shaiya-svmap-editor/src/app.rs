//! The editor window: menus, toolbar, map list, outliner, inspector, problems, changes, status bar, dialogs, and the headless
//! screenshot / frame-time script (`--shot`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Key, RichText, TextureHandle, TextureOptions};
use shaiya_clientdata::wld_raw::Codepage;
use shaiya_clientdata::SysMsg;
use shaiya_svmap::{validate, validate_cross, Finding, SectionId, Severity, Svmap};

use crate::assets::{self, MapAssets, Picture, TintCache};
use crate::client_rules::ClientRules;
use crate::commands::{self, Cmd};
use crate::doc::{self, Doc};
use crate::help::{self, HelpState};
use crate::help_text;
use crate::layout::{self, DockTab, PanelId};
use crate::log;
use crate::palette::{self, MapEntry, Mode as PalMode, Palette};
use crate::prefs::{self, Prefs};
use crate::setup::{self, Wizard, WizardEvent};
use crate::edit::{self, Op, Rec};
use crate::inspector::{self, InspCtx, InspReq};
use crate::names::{self, Lookups};
use crate::pair;
use crate::settings::Settings;
use crate::view2d::{self, Ctx, DrawTarget, PaintSettings, Request, Textures, Tool, VpState};

#[path = "app_tools.rs"]
pub mod tools;
use tools::{GenWin, NewMapWin, SyncWin};
#[path = "app_safety.rs"]
pub mod safety;
use safety::Safety;
#[path = "app_r.rs"]
pub mod r1;
use r1::R1;
#[path = "app_q3.rs"]
pub mod q3;
use q3::Q3;
#[path = "app_spawns.rs"]
pub mod spawn_ui;
use spawn_ui::R2;
#[path = "app_r3.rs"]
pub mod r3_ui;
use r3_ui::R3;
#[path = "app_world.rs"]
pub mod r5_ui;
use r5_ui::{Showing, R5};
#[path = "app_c1.rs"]
pub mod c1_ui;
#[path = "app_c2.rs"]
pub mod c2_ui;
#[path = "app_c3.rs"]
pub mod c3_ui;
#[path = "app_c4.rs"]
pub mod c4_ui;
#[path = "app_c5.rs"]
pub mod c5_ui;
#[path = "app_c6.rs"]
pub mod c6_ui;
#[path = "app3d.rs"]
pub mod three_d;
use three_d::Host3d;
#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

/// Where the app was started from (command line).
#[derive(Default)]
pub struct StartOptions {
    pub files: Vec<PathBuf>,
    pub folder: Option<PathBuf>,
    pub client: Option<PathBuf>,
    pub maps: Vec<u32>,
    /// `+name` / `-name` layer switches.
    pub layer_edits: Vec<(String, bool)>,
    /// `--prop-distance`: cut-off of the 3D buildings, shapes and trees.
    pub prop_distance: Option<f32>,
    /// `--help-page <n>`: start with the Help window open on that page (same as `--show help:<n>`).
    pub help_page: Option<usize>,
    /// `--data-dir <dir>`: settings, log, recovery and backups live here instead of `%APPDATA%\svmap-editor`.
    pub data_dir: Option<PathBuf>,
    /// `--show <setup[:1-3]|help[:page]|palette|quickopen|goto|shortcuts|prefs>`: open that window at start (for screenshots).
    pub show: Option<String>,
    pub shot: Option<ShotOpts>,
    /// Do not read or write the user's settings file (headless runs).
    pub no_settings: bool,
    /// No data folder could be determined (and `--no-settings` was not given): the status bar says settings and backups are off.
    pub no_data_folder: bool,
    pub window: Option<[f32; 2]>,
    /// Start with a new map made from this client map (`--new-from-client`).
    pub new_from_client: Option<u32>,
    /// `--theme dark|light`: the theme for this run (the setting is not changed).
    pub theme: Option<crate::settings::Theme>,
}

pub struct ShotOpts {
    pub out: PathBuf,
    pub perf: bool,
    pub view: Option<(f32, f32, f32)>,
    pub select: Option<(SectionId, usize)>,
    pub tool: Option<Tool>,
    /// Brush dabs applied before the picture: world (x, z), radius in cells, block (true) or free.
    pub paint: Vec<([f32; 2], f32, bool)>,
    /// Open Generate blocked cells with this algorithm and compute the preview.
    pub generate: Option<tools::GenAlgo>,
    /// Open Sync from client.
    pub sync: bool,
    /// Start in the 3D view (`--3d`).
    pub view3d: bool,
    /// 3D camera: yaw and pitch in degrees, then optionally the distance and the focus x, z (`--cam`).
    pub cam: Option<Vec<f32>>,
    /// Scripted mouse drag in the 3D view (`--drag3d section:index:dx,dy`): press on that record, move by (dx, dy) pixels, release.
    pub drag3d: Option<(SectionId, usize, [f32; 2])>,
}

struct FolderMap {
    path: PathBuf,
    svmap: Option<Arc<Svmap>>,
}

struct Folder {
    maps: BTreeMap<u32, FolderMap>,
    rx: Option<Receiver<(u32, Option<Svmap>)>>,
}

enum Slot {
    Loading(Receiver<MapAssets>),
    Ready(Box<Ready>),
}

struct Ready {
    assets: Arc<MapAssets>,
    terrain: Option<TextureHandle>,
    minimap: Option<TextureHandle>,
    dungeon: Option<TextureHandle>,
}

enum Pending {
    AddPortal([f32; 3]),
    Delete,
    Paste(Vec<Rec>, Place),
    MovePortal(usize, bool),
    /// Reverts from the Changes tab that add or remove portals or regions (the client mirrors their count and order).
    Revert { ops: Vec<Op>, label: String },
    /// "Fix all N" of one rule.
    FixAll { ops: Vec<Op>, label: String, code: String, count: usize },
}

/// Where pasted records land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// The first record's lower-left corner at this point.
    Cursor([f32; 2]),
    /// 4 units to the side (Duplicate, and a paste with no cursor in the viewport).
    Offset,
    /// Exactly where the records were copied from (Ctrl+Shift+V).
    Source,
}

enum Then {
    CloseTab,
}

enum Dialog {
    Error(String),
    Unsaved { uid: u64, then: Then },
    UnsavedExit { names: Vec<String> },
    SaveCheck { uid: u64, target: Option<PathBuf>, errors: Vec<String>, disk_changed: bool },
    Confirm { uid: u64, title: String, text: String, action: Pending },
    /// The backup before a save failed: save without one, or stop.
    BackupFailed { uid: u64, target: Option<PathBuf>, why: String },
}

enum PickKind {
    Mob { area: usize, entry: usize },
    Npc { npc: usize },
}

struct Picker {
    uid: u64,
    kind: PickKind,
    query: String,
}

struct PickDest {
    src_uid: u64,
    portal: usize,
    dest_map: u32,
}

struct PerfRow {
    name: String,
    frames: usize,
    avg: f32,
    p95: f32,
    max: f32,
    ui_avg: f32,
}

enum ShotStage {
    Wait { settled: u32 },
    Perf { scenario: usize, frames: Vec<f32>, ui: Vec<f32>, warm: u32 },
    Capture { waited: u32, requested: bool },
}

struct Shot {
    opts: ShotOpts,
    stage: ShotStage,
    rows: Vec<PerfRow>,
    image: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
    done: Arc<AtomicBool>,
}

pub struct App {
    settings: Settings,
    persist: bool,
    /// The status bar says that no data folder could be found.
    no_data_folder: bool,
    /// The data folder (settings, log, recovery, backups); None for `--no-settings` runs.
    data_dir: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    /// The Help window (open flag and state; the last page is kept for the session).
    help_open: bool,
    help: HelpState,
    shortcuts_open: bool,
    wizard: Option<Wizard>,
    palette: Option<Palette>,
    prefs: Option<Prefs>,
    /// A position Go to just moved to: (tab uid, x, z, when). Drawn as a crosshair for three seconds.
    crosshair: Option<(u64, [f32; 2], Instant)>,
    /// The last cursor position in the viewport (the status bar keeps showing it, dimmed, when the mouse leaves).
    last_hover: Option<(u64, [f32; 2])>,
    last_known_window: [f32; 2],
    focus_outliner: bool,
    /// Bumped by Reset layout so the panels forget their sizes.
    layout_gen: u32,
    applied_look: Option<(f32, crate::settings::Theme)>,
    /// `--theme`: wins over the setting for this run.
    theme_override: Option<crate::settings::Theme>,
    last_saved: Settings,
    docs: Vec<Doc>,
    active: usize,
    next_uid: u64,
    folder: Option<Folder>,
    client: Option<Arc<ClientRules>>,
    /// What does not read on this client as it does on US EP4.5 (`episode::client_notes`), shown in the client chip.
    client_notes: Vec<String>,
    client_path: Option<PathBuf>,
    sysmsg: Option<Arc<SysMsg>>,
    tints: Arc<TintCache>,
    slots: std::collections::HashMap<u32, Slot>,
    blocked_tex: std::collections::HashMap<u64, TextureHandle>,
    kinds: std::collections::HashMap<u32, bool>,
    kinds_rx: Option<Receiver<(u32, Option<bool>)>>,
    kinds_started: bool,
    clipboard: Vec<Rec>,
    tool: Tool,
    draw_target: DrawTarget,
    npc_template: (u32, u32),
    drop_on_move: bool,
    paint: PaintSettings,
    gen: Option<GenWin>,
    newmap: Option<NewMapWin>,
    /// The Map.ini editor, the Add a map window and the rebirth pick (C5).
    c5: c5_ui::C5,
    /// The Share client changes window (C6).
    c6: c6_ui::C6,
    sync: Option<SyncWin>,
    vp: VpState,
    /// The wgpu state, for the 3D view.
    rs: Option<egui_wgpu::RenderState>,
    mode3d: bool,
    v3: Host3d,
    /// Tab presses taken out of the input before egui could use them for focus (see `raw_input_hook`).
    tab_pressed: bool,
    /// The scripted drag of `--drag3d`, while it runs.
    script3d: Option<three_d::Script3d>,
    toast: Option<(String, Instant)>,
    dialog: Option<Dialog>,
    picker: Option<Picker>,
    pick_dest: Option<PickDest>,
    map_filter: String,
    outliner_filter: String,
    collapsed: HashSet<SectionId>,
    /// Panels, toolbar, tool palette, layouts (`app_r.rs`).
    r1: R1,
    /// The heatmap and the Spawns tab (`app_spawns.rs`).
    r2: R2,
    /// The Scatter tool and Bulk edit (`app_r3.rs`).
    r3: R3,
    /// The World view and Compare folders (`app_world.rs`).
    r5: R5,
    /// The Placements tool (C3).
    pub(crate) place: crate::place_ui::PlaceUi,
    pub(crate) terrain: crate::terrain_ui::TerrainUi,
    /// Per tab with terrain edits: the redrawn picture and the ground the tab sees.
    terr_views: std::collections::HashMap<u64, c4_ui::TerrView>,
    show_sev: [bool; 3],
    selected_map_row: Option<u32>,
    last_frame: Instant,
    frame_ms: f32,
    ui_ms: f32,
    raw_dt: f32,
    raw_ui: f32,
    frame_no: u64,
    shot: Option<Shot>,
    close_confirmed: bool,
    last_settings_write: Instant,
    /// Autosave and recovery, compare, fixes, disk changes, backups (`app_safety.rs`).
    safety: Safety,
    /// Find, bookmarks, templates, portal links, exports (`app_q3.rs`).
    q3: Q3,
    /// The record a row of the outliner, Problems or Find is hovered over this frame (the viewport outlines it).
    hover_rec: Option<doc::Sel>,
}

fn adapter_line(cc: &eframe::CreationContext<'_>) -> String {
    match cc.wgpu_render_state.as_ref() {
        Some(rs) => {
            let i = rs.adapter.get_info();
            format!("{} | {:?}", i.name, i.backend)
        }
        None => "no wgpu".to_string(),
    }
}

impl App {
    /// The app for the real window: the data folder is `opts.data_dir` (resolved by `main`: `--data-dir`, else the toolkit data
    /// folder); `--no-settings` (and `--shot` without a data folder) means none.
    pub fn new(cc: &eframe::CreationContext<'_>, opts: StartOptions) -> App {
        let data_dir = if opts.no_settings { None } else { opts.data_dir.clone() };
        let mut app = App::new_with(&cc.egui_ctx, adapter_line(cc), opts, data_dir);
        app.rs = cc.wgpu_render_state.clone();
        app
    }

    /// The app without a window: tests and the screenshot runs drive `frame` through an `egui::Context`. `data_dir` None means no
    /// settings, session, recovery, backups, log or setup wizard.
    pub fn new_with(ctx: &egui::Context, adapter: String, opts: StartOptions, data_dir: Option<PathBuf>) -> App {
        let settings_path = data_dir.as_deref().map(crate::settings::file_in);
        let (mut settings, migrated) = settings_path.as_deref().map_or_else(|| (Settings::default(), false), Settings::load_report);
        for (name, on) in &opts.layer_edits {
            set_layer(&mut settings, name, *on);
        }
        if let Some(d) = opts.prop_distance {
            settings.prop_distance = d;
        }
        println!("adapter: {adapter}");
        log::info(format!("start; adapter {adapter}"));
        let persist = data_dir.is_some();
        let mut app = App {
            persist,
            no_data_folder: opts.no_data_folder,
            data_dir,
            settings_path,
            help_open: false,
            help: HelpState::default(),
            shortcuts_open: false,
            wizard: None,
            palette: None,
            prefs: None,
            crosshair: None,
            last_hover: None,
            last_known_window: [0.0, 0.0],
            focus_outliner: false,
            layout_gen: 0,
            applied_look: None,
            theme_override: opts.theme,
            last_saved: settings.clone(),
            settings,
            docs: Vec::new(),
            active: 0,
            next_uid: 1,
            folder: None,
            client: None,
            client_notes: Vec::new(),
            client_path: None,
            sysmsg: None,
            tints: Arc::new(TintCache::default()),
            slots: Default::default(),
            blocked_tex: Default::default(),
            kinds: Default::default(),
            kinds_rx: None,
            kinds_started: false,
            clipboard: Vec::new(),
            tool: Tool::Select,
            draw_target: DrawTarget::MobArea,
            npc_template: (1, 1),
            drop_on_move: false,
            paint: PaintSettings::default(),
            gen: None,
            newmap: None,
            c5: Default::default(),
            c6: Default::default(),
            sync: None,
            vp: VpState::default(),
            rs: None,
            mode3d: opts.shot.as_ref().is_some_and(|s| s.view3d),
            v3: Host3d::default(),
            tab_pressed: false,
            script3d: opts.shot.as_ref().and_then(|s| s.drag3d).map(three_d::Script3d::new),
            toast: None,
            dialog: None,
            picker: None,
            pick_dest: None,
            map_filter: String::new(),
            outliner_filter: String::new(),
            collapsed: HashSet::new(),
            r1: R1::default(),
            r2: R2::default(),
            r3: R3::default(),
            r5: R5::default(),
            place: Default::default(),
            terrain: Default::default(),
            terr_views: Default::default(),
            show_sev: [true, true, false],
            selected_map_row: None,
            last_frame: Instant::now(),
            frame_ms: 16.0,
            ui_ms: 0.0,
            raw_dt: 16.0,
            raw_ui: 0.0,
            frame_no: 0,
            shot: None,
            close_confirmed: false,
            last_settings_write: Instant::now(),
            safety: Safety::new(),
            q3: Q3::default(),
            hover_rec: None,
        };
        let cmdline = !opts.files.is_empty() || opts.folder.is_some() || opts.client.is_some() || !opts.maps.is_empty() || opts.new_from_client.is_some();
        let had_folders = app.settings.svmap_folder.is_some() || app.settings.client_folder.is_some();
        // startup: client first (names), then folder, then files
        let client = opts.client.clone().or_else(|| app.settings.client_folder.as_ref().filter(|_| app.persist).map(PathBuf::from));
        if let Some(c) = client {
            app.open_client(&c, ctx);
        }
        let folder = opts.folder.clone().or_else(|| app.settings.svmap_folder.as_ref().filter(|_| app.persist && opts.files.is_empty() && opts.maps.is_empty()).map(PathBuf::from));
        if let Some(f) = folder {
            if f.is_dir() {
                app.open_folder(&f, ctx);
            }
        }
        for id in &opts.maps {
            app.open_map_id(*id);
        }
        for f in &opts.files {
            app.open_file(f);
        }
        if let Some(id) = opts.new_from_client {
            match app.new_info(id) {
                Ok(i) => app.create_from_client(id, i.size, true, false),
                Err(e) => app.dialog = Some(Dialog::Error(e)),
            }
        }
        // unsaved work of an earlier session is offered back before anything else (a session tab for the same file waits for it)
        if app.persist {
            app.scan_recovery();
        }
        // session of the last run, only for a plain start
        if app.persist && !cmdline && app.settings.session.restore {
            app.restore_session();
        }
        // first run: the setup wizard (people who already have folders from an older build skip it)
        if app.persist && !app.settings.first_run_done {
            if cmdline || had_folders {
                app.settings.first_run_done = true;
            } else {
                app.open_setup();
            }
        }
        if migrated {
            app.toast("New layout: panels open when needed. View > Layout > Review keeps Problems open.");
        }
        if let Some(p) = opts.help_page {
            app.help_open = true;
            app.help.page = p;
        }
        if let Some(spec) = opts.show.as_deref() {
            app.show_ui(spec);
        }
        if let Some(s) = opts.shot {
            app.shot = Some(Shot { opts: s, stage: ShotStage::Wait { settled: 0 }, rows: Vec::new(), image: Arc::new(Mutex::new(None)), done: Arc::new(AtomicBool::new(false)) });
        }
        app
    }

    /// `--show`: opens one window at start so a screenshot can capture it.
    fn show_ui(&mut self, spec: &str) {
        let (name, arg) = spec.split_once(':').unwrap_or((spec, ""));
        match name {
            "setup" => {
                self.open_setup();
                if let Some(w) = self.wizard.as_mut() {
                    w.step = arg.parse::<usize>().map_or(0, |n| n.saturating_sub(1).min(2));
                }
            }
            "help" => {
                self.help_open = true;
                if !arg.is_empty() {
                    self.help.goto_named(arg);
                }
            }
            "palette" => self.palette = Some(Palette::new(PalMode::Commands)),
            "quickopen" => {
                let mut p = Palette::new(PalMode::Maps);
                p.query = arg.to_string();
                self.palette = Some(p);
            }
            "goto" => {
                let mut p = Palette::new(PalMode::GoTo);
                p.query = arg.to_string();
                self.palette = Some(p);
            }
            "shortcuts" => self.shortcuts_open = true,
            "prefs" => {
                let mut p = Prefs::new(&self.settings);
                if arg == "folders" {
                    p.tab = prefs::Tab::Folders;
                }
                self.prefs = Some(p);
            }
            "problems" => self.demo_problems(),
            "pair" => self.demo_pair(arg == "sync"),
            "place" => self.demo_place(),
            "terrain" => self.demo_terrain(),
            "ini" => self.demo_ini(arg),
            "add-map" => self.demo_add_map(arg),
            "share" => self.demo_share(arg),
            "compare" => self.demo_compare(),
            "recovery" => self.demo_recovery(),
            "find" => {
                self.show_find(false);
                self.q3.find.query = arg.to_string();
            }
            "links" => self.q3.links_open = true,
            "batch" => self.demo_batch(),
            "layout" => {
                if layout::find(arg, &self.settings.layouts).is_some() {
                    self.apply_layout(arg);
                } else {
                    eprintln!("--show layout: unknown layout {arg} (Simple, Editing, Review or a saved one)");
                }
            }
            "dock" => match arg {
                "problems" => self.open_dock(DockTab::Problems),
                "problems-info" => {
                    self.show_sev[2] = true;
                    self.open_dock(DockTab::Problems);
                }
                "changes" => self.open_dock(DockTab::Changes),
                "find" => self.open_dock(DockTab::Find),
                "spawns" => self.open_dock(DockTab::Spawns),
                _ => eprintln!("--show dock: use problems, problems-info, changes, find or spawns"),
            },
            "collapsed" => match arg {
                "browser" => self.settings.layout.set_collapsed(PanelId::Browser, true),
                "inspector" => self.settings.layout.set_collapsed(PanelId::Inspector, true),
                "dock" => {
                    self.settings.layout.dock.visible = true;
                    self.settings.layout.set_collapsed(PanelId::Dock, true);
                }
                _ => eprintln!("--show collapsed: use browser, inspector or dock"),
            },
            "viewport-only" => self.run_layout_cmd(Cmd::ViewportOnly),
            "heat" => match arg {
                "density" => self.settings.heat.mode = crate::heat::Mode::Density,
                "level" => self.settings.heat.mode = crate::heat::Mode::Level,
                _ => eprintln!("--show heat: use density or level"),
            },
            "spawns" => {
                self.spawn_check();
                self.r2.ui.all_maps = arg.starts_with("folder");
                self.r2.ui.view = match arg {
                    "folder:findings" => spawn_ui::FolderView::Findings,
                    "folder:never" => spawn_ui::FolderView::Never,
                    _ => spawn_ui::FolderView::Maps,
                };
            }
            "scatter" => {
                // a rectangle of the map's middle with the monsters of its first area, for a picture of the preview
                if let Some(d) = self.docs.get(self.active) {
                    let c = d.map.size as f32 / 2.0;
                    let mut mobs: Vec<[u32; 2]> = Vec::new();
                    for m in d.map.mob_areas.iter().flat_map(|a| a.mobs.iter().map(|e| e.mob)) {
                        if mobs.len() < 2 && !mobs.iter().any(|x| x[0] == m) {
                            mobs.push([m, 1]);
                        }
                    }
                    self.settings.scatter.mobs = if mobs.is_empty() { vec![[1, 1]] } else { mobs };
                    self.tool = Tool::Scatter;
                    let r = if arg == "poly" { crate::scatter::Region::poly(vec![[c - 160.0, c - 120.0], [c + 160.0, c - 120.0], [c + 160.0, c], [c, c], [c, c + 140.0], [c - 160.0, c + 140.0]]) } else { crate::scatter::Region::rect([c - 160.0, c - 120.0], [c + 160.0, c + 120.0]) };
                    self.vp.scatter.set(r);
                }
            }
            "measure" => {
                // a path over the middle of the map (`measure:radius` a circle of the portal range) for a picture
                if let Some(d) = self.docs.get(self.active) {
                    let c = d.map.size as f32 / 2.0;
                    self.tool = Tool::Measure;
                    if arg == "radius" {
                        self.vp.measure.circle_to([c, c], [c + crate::measure::RING, c]);
                    } else {
                        for p in [[c - 150.0, c - 80.0], [c - 60.0, c + 30.0], [c + 50.0, c + 10.0], [c + 130.0, c + 110.0]] {
                            self.vp.measure.click(p);
                        }
                        self.vp.measure.finish();
                    }
                }
            }
            "rulers" => self.settings.rulers = true,
            "world" => {
                self.open_world();
                if arg == "links" {
                    self.r5.world.pending_arrange = Some(r5_ui::Arrange::ByLinks);
                }
                let (id, zoom) = arg.split_once('@').unwrap_or((arg, ""));
                if let Ok(id) = id.parse::<u32>() {
                    self.r5.world.selected = Some(id);
                    if let Ok(z) = zoom.parse::<f32>() {
                        self.r5.world.focus = Some((id, z));
                    }
                }
            }
            "folderdiff" => {
                self.r5.fd.other = arg.to_string();
                self.open_compare_folders();
                self.r5.fd.other = arg.to_string();
            }
            "bulk" => self.open_bulk(Some(if arg.is_empty() { "map:1-5 mob:12 => count +1".to_string() } else { arg.to_string() })),
            other => eprintln!("--show: unknown window {other} (setup, help, palette, quickopen, goto, shortcuts, prefs[:folders], problems, compare, recovery, find[:query], links, batch, layout:<name>, dock:<tab>, collapsed:<panel>, viewport-only, heat:density|level, spawns[:folder|:folder:findings|:folder:never], scatter[:poly], bulk[:rule], measure[:radius], rulers, world[:<map id>[@<zoom>]|:links], folderdiff:<folder>)"),
        }
    }

    /// `--show problems`: breaks a few things in the open map (a box the wrong way round, counts, a portal) so the Problems tab
    /// shows findings with their Fix buttons, grouped by rule.
    fn demo_problems(&mut self) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let mut ops = Vec::new();
        for i in [0usize, 1, 2, 3, 4, 5].into_iter().filter(|i| *i < d.map.mob_areas.len()) {
            if let Some(Rec::Mob(a)) = edit::get(&d.map, SectionId::MobAreas, i) {
                let before = Rec::Mob(a.clone());
                let mut b = a;
                match i {
                    0 | 1 => std::mem::swap(&mut b.min[0], &mut b.max[0]),
                    2 => b.mobs.iter_mut().for_each(|e| e.count = 12),
                    3 => b.mobs.iter_mut().for_each(|e| e.count = 0),
                    4 => b.mobs.clear(),
                    _ => b.min[0] = -9.0,
                }
                ops.push(Op::replace(i, before, Rec::Mob(b)));
            }
        }
        if let Some(Rec::Portal(p)) = edit::get(&d.map, SectionId::Portals, 0) {
            let mut q = p;
            q.level_min = 50;
            q.level_max = 10;
            ops.push(Op::replace(0, Rec::Portal(p), Rec::Portal(q)));
        }
        d.commit("demo: break a few records", ops);
        self.open_dock(DockTab::Problems);
        self.show_sev = [true, true, true];
        self.safety.group_problems = true;
    }

    /// `--show pair`: a paired edit (portal 0 moves 6 units, region 3 is renamed) on a tab with its client half, for the picture of
    /// the chain icons and the unsaved chip. `--show pair:sync` also drifts the two sides apart and opens Sync from client.
    fn demo_pair(&mut self, with_sync: bool) {
        let Some(uid) = self.docs.get(self.active).map(|d| d.uid) else { return };
        self.attach_client_now(uid);
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if d.part().is_none() {
            eprintln!("--show pair needs a map with an editable client .wld (open the client folder)");
            return;
        }
        if let Some(before) = edit::get(&d.map, SectionId::Portals, 0) {
            let mut after = before.clone();
            edit::translate(&mut after, 6.0, 0.0);
            d.commit("move portal", vec![Op::replace(0, before, after)]);
        }
        let name = d.part().and_then(|p| p.names().get(3).map(|_| p.txt.clone()));
        if let Some(txt) = name {
            if let Ok(after) = crate::pair::txt_with_name(&txt, 3, "Mine Entrance") {
                let _ = d.commit_client("rename region #3", vec![crate::pair::COp::Txt { before: txt, after }]);
            }
        }
        if with_sync {
            if let Some(Rec::Portal(p)) = edit::get(&d.map, SectionId::Portals, 1) {
                let mut q = p;
                q.level_max = q.level_max.saturating_add(20);
                d.commit_server_only("svmap portal 1", vec![Op::replace(1, Rec::Portal(p), Rec::Portal(q))]);
            }
            if let Some(c) = d.part().and_then(|p| p.wld.model.portals.get(2).cloned()) {
                let mut n = c.clone();
                n.dest_map += 1;
                let _ = d.commit_client("client portal 2", vec![crate::pair::COp::Portal(2, crate::pair::CKind::Replace { before: c, after: n })]);
            }
            if let Some(Rec::Npc(n)) = edit::get(&d.map, SectionId::Npcs, 5) {
                let mut m = n.clone();
                if let Some(p) = m.points.first_mut() {
                    p.pos[0] += 4.0;
                }
                d.commit_server_only("svmap NPC 5", vec![Op::replace(5, Rec::Npc(n), Rec::Npc(m))]);
            }
        }
        d.select_only(SectionId::Portals, 0);
        if with_sync {
            self.open_sync();
        }
    }

    /// `--show place`: the Placements tool with a building turned by 20 degrees and selected, and the Models window previewing its model.
    fn demo_place(&mut self) {
        let Some(uid) = self.docs.get(self.active).map(|d| d.uid) else { return };
        self.attach_client_now(uid);
        self.tool = Tool::Place;
        let rules = self.client.clone();
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let Some(part) = d.part() else {
            eprintln!("--show place needs a map with an editable client .wld (open the client folder)");
            return;
        };
        let m = &part.wld.model;
        let c = d.map.size as f32 * 0.5;
        let mut best: Vec<(f32, usize)> = m.building.items.iter().enumerate().filter(|(_, p)| p.index >= 0).map(|(i, p)| ((p.pos[0] - c).hypot(p.pos[2] - c), i)).collect();
        best.sort_by(|a, b| a.0.total_cmp(&b.0));
        let first = best.first().map(|b| b.1);
        let name = first.and_then(|i| crate::place::model_name(&m.building, &m.building.items[i]));
        if let Some(i) = first {
            let ops = crate::place::rotate_ops(m, &[(crate::place::PlaceList::Building, i)], 20.0);
            let q = m.building.items[i].pos;
            let _ = d.commit_client("turn placement", ops);
            d.place_sel = vec![(crate::place::PlaceList::Building, i)];
            d.view = crate::doc::View { cx: q[0] + 45.0, cz: q[2], zoom: 4.0, fitted: true };
        }
        self.place.browser_open = true;
        self.place.list = crate::place::PlaceList::Building;
        self.place.filter = String::new();
        // the Models window previews the selected building's model
        self.place.preview = name.and_then(|n| crate::place_ui::build_preview(rules.as_ref().map(|r| &r.source), crate::place::PlaceList::Building, &n));
    }

    /// `--show compare`: a few edits, the Changes tab and the compare layer.
    fn demo_compare(&mut self) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let n = d.map.mob_areas.len();
        if n > 12 {
            if let Some(before) = edit::get(&d.map, SectionId::MobAreas, 10) {
                let mut after = before.clone();
                edit::translate(&mut after, 25.0, -18.0);
                d.commit("move area", vec![Op::replace(10, before, after)]);
            }
            d.selection = vec![(SectionId::MobAreas, 6), (SectionId::MobAreas, 7)];
            d.delete_selected();
            if let Some(Rec::Mob(a)) = edit::get(&d.map, SectionId::MobAreas, 3) {
                let mut b = a;
                b.min[0] += 40.0;
                b.max[0] += 60.0;
                let at = d.map.mob_areas.len();
                d.commit("add area", vec![Op::insert(at, Rec::Mob(b))]);
            }
            let mut g = d.map.blocked.clone();
            let s = d.map.size.min(400) as i64;
            g.fill_rect(s / 4, s / 4, s / 4 + 30, s / 4 + 12, true);
            d.commit_blocked("paint", &g);
            d.selection.clear();
        }
        self.open_dock(DockTab::Changes);
        self.settings.layers.compare = true;
    }

    /// `--show batch`: selects the first monster areas of the open map, to show the batch edit panel.
    fn demo_batch(&mut self) {
        if let Some(d) = self.docs.get_mut(self.active) {
            d.selection = (0..d.map.mob_areas.len().min(5)).map(|i| (SectionId::MobAreas, i)).collect();
        }
    }

    /// `--show recovery`: writes a recovery entry for the open map (as if the editor had stopped with unsaved work) and shows
    /// the dialog that offers it back. Needs `--data-dir`.
    fn demo_recovery(&mut self) {
        let Some(data) = self.data_dir.clone() else {
            eprintln!("--show recovery needs --data-dir");
            return;
        };
        let Some(i) = (!self.docs.is_empty()).then_some(self.active) else { return };
        self.demo_compare();
        let d = &self.docs[i];
        let now = log::unix_ms(std::time::SystemTime::now());
        let mut meta = safety::meta_for(d, now - 190_000);
        meta.heartbeat_unix_ms = 0;
        let key = crate::recovery::entry_key(d.path.as_deref(), d.uid);
        if let Err(e) = crate::recovery::write_entry(&data, &key, &d.map, &meta) {
            eprintln!("--show recovery: {e}");
            return;
        }
        // the entry is on disk; the tab goes (clean), as after a restart
        let uid = d.uid;
        self.safety.forget(uid);
        self.docs.remove(i);
        self.active = 0;
        self.open_dock(DockTab::Problems);
        self.scan_recovery();
    }

    fn open_setup(&mut self) {
        let s = &self.settings;
        self.wizard = Some(Wizard::with_paths(s.svmap_folder.as_deref(), s.client_folder.as_deref(), s.reference_folder.as_deref()));
    }

    // ------------------------------------------------------------------------------------------------------------ opening

    fn new_uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn open_file(&mut self, path: &Path) {
        self.r5.showing = Showing::Map;
        if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(path) && !d.read_only) {
            self.active = i;
            return;
        }
        let uid = self.new_uid();
        match Doc::open(uid, path) {
            Ok(d) => {
                log::info(format!("opened {}", path.display()));
                self.docs.push(d);
                self.active = self.docs.len() - 1;
                self.settings.add_recent(&path.display().to_string());
                self.vp = VpState::default();
            }
            Err(e) => {
                log::error(format!("cannot open {}: {e}", path.display()));
                self.dialog = Some(Dialog::Error(e));
            }
        }
    }

    fn open_map_id(&mut self, id: u32) {
        let path = self.folder.as_ref().and_then(|f| f.maps.get(&id)).map(|m| m.path.clone());
        match path {
            Some(p) => self.open_file(&p),
            None => self.toast(format!("map {id} is not in the open folder")),
        }
    }

    fn open_folder(&mut self, dir: &Path, ctx: &egui::Context) {
        let mut maps = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("svmap")) {
                    if let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
                        maps.insert(id, FolderMap { path: p, svmap: None });
                    }
                }
            }
        }
        if maps.is_empty() {
            log::error(format!("{} holds no <id>.svmap files", dir.display()));
            self.dialog = Some(Dialog::Error(format!("{} holds no <id>.svmap files", dir.display())));
            return;
        }
        log::info(format!("opened folder {} ({} maps)", dir.display(), maps.len()));
        let (tx, rx) = channel();
        let list: Vec<(u32, PathBuf)> = maps.iter().map(|(k, v)| (*k, v.path.clone())).collect();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            for (id, p) in list {
                let m = std::fs::read(&p).ok().and_then(|b| shaiya_svmap::read(&b).ok());
                if tx.send((id, m)).is_err() {
                    return;
                }
                ctx2.request_repaint();
            }
        });
        self.settings.svmap_folder = Some(dir.display().to_string());
        self.settings.add_recent(&dir.display().to_string());
        self.folder = Some(Folder { maps, rx: Some(rx) });
        self.kinds_started = false;
        self.kinds.clear();
        self.mark_all_for_validation();
    }

    fn open_client(&mut self, dir: &Path, ctx: &egui::Context) {
        match ClientRules::open(dir) {
            Ok(r) => {
                self.adopt_client(dir, r);
                let _ = ctx;
            }
            Err(e) => {
                log::error(format!("cannot open client {}: {e}", dir.display()));
                self.dialog = Some(Dialog::Error(e));
            }
        }
    }

    /// Makes `r` the open client: logs its problems and episode notes once, keeps the notes for the client chip and resets what
    /// depends on the client.
    fn adopt_client(&mut self, dir: &Path, r: ClientRules) {
        log::info(format!("opened client {} ({}; {} problems)", dir.display(), if r.source.is_archive() { "archive" } else { "extracted folder" }, r.problems.len()));
        for pr in &r.problems {
            log::error(format!("client problem: {pr}"));
        }
        let notes = shaiya_svmap_tools::episode::client_notes(&r);
        for n in &notes {
            log::info(format!("client note: {n}"));
        }
        self.client_notes = notes;
        self.sysmsg = r.source.sysmsg().ok().map(Arc::new);
        self.client = Some(Arc::new(r));
        self.client_path = Some(dir.to_path_buf());
        self.apply_staging_setting();
        self.settings.client_folder = Some(dir.display().to_string());
        self.slots.clear();
        self.v3.clear_scenes();
        self.q3.forget_client();
        self.kinds_started = false;
        self.kinds.clear();
        self.mark_all_for_validation();
    }

    fn mark_all_for_validation(&mut self) {
        self.safety.invalidate();
        for d in &mut self.docs {
            d.validate_due = Some(Instant::now());
        }
    }

    fn map_name(&self, id: u32) -> String {
        shaiya_clientdata::map_name(id, self.sysmsg.as_deref())
    }

    /// File > Reload client data: reads the client files again (the patcher may have rewritten data.saf); open tabs stay.
    fn reload_client(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.client_path.clone() else { return };
        self.open_client(&dir, ctx);
        if self.client.is_some() {
            self.toast("client data reloaded");
        }
    }

    /// Forget the client folder: names fall back to numbers, the open tabs stay.
    fn clear_client(&mut self) {
        self.client = None;
        self.client_path = None;
        self.sysmsg = None;
        self.client_notes.clear();
        self.settings.client_folder = None;
        self.slots.clear();
        self.v3.clear_scenes();
        self.q3.forget_client();
        self.kinds_started = false;
        self.kinds.clear();
        self.mark_all_for_validation();
    }

    // ------------------------------------------------------------------------------------------------------------ session

    /// The open tabs, cameras, tool and 2D/3D, as saved in the settings.
    fn session_snapshot(&self) -> crate::settings::Session {
        let mut tabs = Vec::new();
        let mut active = 0;
        for (i, d) in self.docs.iter().enumerate() {
            // a backup copy has no map file name of its own: it is not part of the session
            let Some(p) = d.path.as_ref().filter(|_| d.label.is_none()) else { continue };
            if i == self.active {
                active = tabs.len();
            }
            tabs.push(crate::settings::SessionTab { path: p.display().to_string(), view: [d.view.cx, d.view.cz, d.view.zoom], read_only: d.read_only });
        }
        crate::settings::Session { restore: self.settings.session.restore, tabs, active, tool: tool_name(self.tool).to_string(), view_3d: self.mode3d && !self.docs.is_empty() }
    }

    /// Opens the tabs of the last session (files that are gone are named in one toast; a file with unsaved work in the recovery
    /// folder is left to the recovery dialog).
    fn restore_session(&mut self) {
        let sess = self.settings.session.clone();
        let recovered = self.recovery_keys();
        let skipped = |t: &crate::settings::SessionTab| !Path::new(&t.path).is_file() || recovered.contains(&crate::settings::path_key(&t.path));
        let mut gone: Vec<String> = Vec::new();
        for t in &sess.tabs {
            let p = PathBuf::from(&t.path);
            if !p.is_file() {
                gone.push(p.file_name().map_or(t.path.clone(), |n| n.to_string_lossy().into_owned()));
                continue;
            }
            if skipped(t) {
                continue;
            }
            if t.read_only {
                let id = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
                self.open_read_only(&p, None, id);
            } else {
                self.open_file(&p);
            }
            if let Some(d) = self.docs.iter_mut().find(|d| d.path.as_deref() == Some(p.as_path()) && d.read_only == t.read_only) {
                d.view = crate::doc::View { cx: t.view[0], cz: t.view[1], zoom: t.view[2].clamp(view2d::MIN_ZOOM, view2d::MAX_ZOOM), fitted: true };
            }
        }
        if !self.docs.is_empty() {
            // the active index counts the tabs that were restored, in order
            let lost_before = sess.tabs.iter().take(sess.active).filter(|t| skipped(t)).count();
            self.active = sess.active.saturating_sub(lost_before).min(self.docs.len() - 1);
            self.vp = VpState::default();
        }
        self.tool = tool_from_name(&sess.tool);
        if sess.view_3d && !self.docs.is_empty() {
            self.set_mode3d(true);
        }
        if !gone.is_empty() {
            self.toast(format!("{} map{} from the last session {} gone: {}", gone.len(), if gone.len() == 1 { "" } else { "s" }, if gone.len() == 1 { "is" } else { "are" }, gone.join(", ")));
        }
    }

    /// Writes the settings now (the debounced write and the exit path both end here).
    fn flush_settings(&mut self) {
        if !self.persist {
            return;
        }
        if self.last_known_window[0] > 0.0 {
            self.settings.window = self.last_known_window;
        }
        self.settings.session = self.session_snapshot();
        let Some(path) = self.settings_path.clone() else { return };
        // viewport only is not saved: a restart shows the layout that was stored
        let mut out = self.settings.clone();
        if let Some(stored) = self.r1.vo.stored() {
            out.layout = stored.clone();
        }
        match out.save_to(&path) {
            Ok(()) => self.last_saved = self.settings.clone(),
            Err(e) => log::error(format!("cannot write the settings: {e}")),
        }
    }

    // ------------------------------------------------------------------------------------------------------------ drag and drop, recent, quick open

    /// Files and folders dropped on the window: a `.svmap` opens, a folder of maps becomes the map folder, a client folder or
    /// `data.sah` becomes the client; anything else gets a toast.
    pub(crate) fn handle_dropped(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            match classify_drop(&p) {
                DropKind::Map => self.open_file(&p),
                DropKind::MapFolder => self.open_folder(&p, ctx),
                DropKind::Client(dir) => self.open_client(&dir, ctx),
                DropKind::Unknown => self.toast(format!("{} is not a map file, a map folder or a client folder", p.display())),
            }
        }
    }

    /// The Recent list: pinned entries, then folders, then maps. (path, is a folder, still exists, pinned)
    fn recent_entries(&self) -> Vec<(String, bool, bool, bool)> {
        let mut v: Vec<(String, bool, bool, bool)> = Vec::new();
        for r in &self.settings.recent {
            let p = Path::new(r);
            let is_dir = p.is_dir() || (!p.exists() && p.extension().is_none());
            v.push((r.clone(), is_dir, p.exists(), self.settings.recent_pinned.contains(r)));
        }
        v.sort_by_key(|(_, is_dir, _, pinned)| (!*pinned, !*is_dir));
        v
    }

    /// The text of a Recent entry: a folder as its path, a map as `35 Apulune - path`.
    fn recent_label(&self, path: &str, is_dir: bool) -> String {
        if is_dir {
            return path.to_string();
        }
        match Path::new(path).file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
            Some(id) => format!("{id} {} — {path}", self.map_name(id)),
            None => path.to_string(),
        }
    }

    /// Everything quick-open can offer: the open folder's maps, open tabs and the recent files.
    fn map_entries(&self) -> Vec<MapEntry> {
        let mut v: Vec<MapEntry> = Vec::new();
        let recent: Vec<&String> = self.settings.recent.iter().collect();
        let doc_of = |id: Option<u32>, path: Option<&Path>| self.docs.iter().find(|d| (path.is_some() && d.path.as_deref() == path) || (id.is_some() && d.map_id == id && d.path.is_none()));
        if let Some(f) = &self.folder {
            for (id, m) in &f.maps {
                let d = doc_of(Some(*id), Some(&m.path)).or_else(|| self.docs.iter().find(|d| d.map_id == Some(*id)));
                v.push(MapEntry {
                    id: Some(*id),
                    name: self.map_name(*id),
                    path: Some(m.path.clone()),
                    size: d.map(|d| d.map.size).or_else(|| m.svmap.as_ref().map(|s| s.size)),
                    dungeon: self.kinds.get(id).copied(),
                    open: d.is_some(),
                    modified: d.is_some_and(|d| d.is_dirty()),
                    recent: recent.iter().any(|r| Path::new(r.as_str()) == m.path),
                });
            }
        }
        // tabs that are not in the folder (a file from elsewhere, a new map)
        for d in &self.docs {
            let in_folder = self.folder.as_ref().is_some_and(|f| d.map_id.is_some_and(|id| f.maps.get(&id).is_some_and(|m| Some(&m.path) == d.path.as_ref())));
            if !in_folder {
                v.push(MapEntry { id: d.map_id, name: d.map_id.map(|i| self.map_name(i)).unwrap_or_else(|| d.title()), path: d.path.clone(), size: Some(d.map.size), dungeon: d.map_id.and_then(|i| self.kinds.get(&i).copied()), open: true, modified: d.is_dirty(), recent: true });
            }
        }
        // recent files that are on neither list
        for r in &self.settings.recent {
            let p = Path::new(r);
            if p.is_dir() || !p.exists() || v.iter().any(|e| e.path.as_deref() == Some(p)) {
                continue;
            }
            let id = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
            v.push(MapEntry { id, name: id.map(|i| self.map_name(i)).unwrap_or_default(), path: Some(p.to_path_buf()), recent: true, ..Default::default() });
        }
        v
    }

    fn open_entry(&mut self, e: &MapEntry) {
        if let Some(i) = self.docs.iter().position(|d| (e.path.is_some() && d.path == e.path) || (e.path.is_none() && e.id.is_some() && d.map_id == e.id)) {
            self.switch_to(i);
            return;
        }
        if let Some(p) = &e.path {
            self.open_file(p);
        } else if let Some(id) = e.id {
            self.open_map_id(id);
        }
    }

    fn switch_to(&mut self, i: usize) {
        self.r5.showing = Showing::Map;
        if i < self.docs.len() && i != self.active {
            self.active = i;
            self.vp = VpState::default();
        }
    }

    // ------------------------------------------------------------------------------------------------------------ go to, copy position

    /// Checks a Go to query against the open map.
    fn resolve_goto(&self, query: &str) -> Result<(palette::Resolved, String), String> {
        let g = palette::parse_goto(query)?;
        let d = self.docs.get(self.active);
        let count = |s: SectionId| d.map_or(0, |d| edit::count(&d.map, s));
        let region = |x: f32, z: f32| {
            let d = d?;
            let assets = self.current_assets();
            let look = Lookups { rules: self.client.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
            view2d::region_at(&d.map, x, z).map(|i| look.region_label(i, &d.map.regions[i]))
        };
        let has = |m: u32| self.folder.as_ref().is_some_and(|f| f.maps.contains_key(&m)) || self.docs.iter().any(|d| d.map_id == Some(m));
        let marks = self.bookmarks_here();
        let env = palette::GotoEnv { map_id: d.and_then(|d| d.map_id), size: d.map(|d| d.map.size), count: &count, bookmarks: &marks, region: &region, has_map: &has };
        palette::resolve(&g, &env)
    }

    /// Bookmarks of the open map (Q3 adds a way to make them; Go to already reads them).
    fn bookmarks_here(&self) -> Vec<crate::settings::Bookmark> {
        let Some(d) = self.docs.get(self.active) else { return Vec::new() };
        let key = format!("{}#{}", crate::settings::path_key(&self.settings.svmap_folder.clone().unwrap_or_default()), d.map_id.unwrap_or(0));
        self.settings.bookmarks.get(&key).cloned().unwrap_or_default()
    }

    /// Does what a resolved Go to says: open a map, select and frame a record, or centre the view (2D) or the camera focus (3D)
    /// on a position and mark it with a crosshair for three seconds.
    fn do_goto(&mut self, r: palette::Resolved) {
        match r {
            palette::Resolved::OpenMap(id) => self.open_map_id(id),
            palette::Resolved::Select { section, index } => {
                if let Some(d) = self.docs.get_mut(self.active) {
                    if index < edit::count(&d.map, section) {
                        d.select_only(section, index);
                        d.sub = None;
                    }
                }
                self.frame_selection();
            }
            palette::Resolved::Position { map, x, y, z, zoom } => {
                if let Some(m) = map {
                    self.open_map_id(m);
                    if self.docs.get(self.active).and_then(|d| d.map_id) != Some(m) {
                        return;
                    }
                }
                let ground = self.ground_fn();
                let mode3d = self.mode3d;
                let Some(d) = self.docs.get_mut(self.active) else { return };
                d.view.cx = x;
                d.view.cz = z;
                d.view.zoom = zoom.unwrap_or(d.view.zoom.max(0.5)).clamp(view2d::MIN_ZOOM, view2d::MAX_ZOOM);
                d.view.fitted = true;
                let uid = d.uid;
                if mode3d {
                    let smooth = self.settings.smooth_camera;
                    if let Some(st) = self.v3.states.get_mut(&uid) {
                        let focus = [x, y.unwrap_or_else(|| ground(x, z)), z];
                        if smooth && !st.cam.fly {
                            st.cancel_anim();
                            st.goto = Some(crate::view3d::CamPose { focus, ..st.cam.pose() });
                        } else {
                            st.cam.focus = focus;
                        }
                    }
                }
                self.crosshair = Some((uid, [x, z], Instant::now()));
            }
        }
    }

    /// Where Copy position takes x and z from: the cursor, else the selection's anchor, else the view centre.
    fn position_for_copy(&self) -> Option<[f32; 2]> {
        let d = self.docs.get(self.active)?;
        if let Some(p) = self.vp.hover {
            return Some(p);
        }
        if let Some((s, i)) = d.selection.first() {
            if let Some(r) = edit::get(&d.map, *s, *i) {
                return Some(view2d::anchor_of(&r));
            }
        }
        Some([d.view.cx, d.view.cz])
    }

    fn copy_position(&mut self, ctx: &egui::Context, gm: bool) {
        let Some(p) = self.position_for_copy() else { return };
        let text = if gm {
            let Some(id) = self.docs.get(self.active).and_then(|d| d.map_id) else { return };
            gm_move_text(p, id)
        } else {
            position_text(p)
        };
        ctx.copy_text(text.clone());
        self.toast(format!("copied {text}"));
    }

    // ------------------------------------------------------------------------------------------------------------ background

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(f) = &mut self.folder {
            let mut finished = false;
            if let Some(rx) = &f.rx {
                loop {
                    match rx.try_recv() {
                        Ok((id, m)) => {
                            if let (Some(e), Some(m)) = (f.maps.get_mut(&id), m) {
                                e.svmap = Some(Arc::new(m));
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished = true;
                            break;
                        }
                    }
                }
            }
            if finished {
                f.rx = None;
                self.mark_all_for_validation();
            }
        }
        // F/D kinds, once both the folder and the client are open
        if !self.kinds_started {
            if let (Some(c), Some(f)) = (&self.client, &self.folder) {
                self.kinds_started = true;
                let (tx, rx) = channel();
                let ids: Vec<u32> = f.maps.keys().copied().collect();
                let c = c.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    for id in ids {
                        let k = c.source.wld(id).ok().map(|w| matches!(w.kind, shaiya_clientdata::wld::WldKind::Dungeon));
                        if tx.send((id, k)).is_err() {
                            return;
                        }
                    }
                    ctx2.request_repaint();
                });
                self.kinds_rx = Some(rx);
            }
        }
        if let Some(rx) = &self.kinds_rx {
            let mut got = false;
            while let Ok((id, k)) = rx.try_recv() {
                if let Some(k) = k {
                    self.kinds.insert(id, k);
                }
                got = true;
            }
            if got {
                ctx.request_repaint();
            }
        }
        // asset loads
        let ids: Vec<u32> = self.slots.keys().copied().collect();
        for id in ids {
            let done = match self.slots.get(&id) {
                Some(Slot::Loading(rx)) => rx.try_recv().ok(),
                _ => None,
            };
            if let Some(a) = done {
                let ready = make_ready(ctx, a);
                self.slots.insert(id, Slot::Ready(Box::new(ready)));
                ctx.request_repaint();
            }
        }
        self.poll_scenes(ctx);
    }

    /// Makes sure the assets of the active map are loading or loaded.
    fn ensure_assets(&mut self, ctx: &egui::Context) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(id) = d.map_id else { return };
        if self.slots.contains_key(&id) {
            return;
        }
        let size = d.map.size;
        self.start_assets_load(id, size, ctx);
    }

    /// Starts loading the client assets (terrain, ground, minimap, ...) of map `id` unless a slot exists. Without client data the
    /// slot is an empty one at once.
    pub(super) fn start_assets_load(&mut self, id: u32, size: u32, ctx: &egui::Context) {
        if self.slots.contains_key(&id) {
            return;
        }
        match &self.client {
            None => {
                self.slots.insert(id, Slot::Ready(Box::new(make_ready(ctx, MapAssets::empty()))));
            }
            Some(c) => {
                let (tx, rx) = channel();
                let c = c.clone();
                let tints = self.tints.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let a = assets::load(&c, &tints, id, size);
                    let _ = tx.send(a);
                    ctx2.request_repaint();
                });
                self.slots.insert(id, Slot::Loading(rx));
            }
        }
    }

    fn ready_for(&self, id: Option<u32>) -> Option<&Ready> {
        match self.slots.get(&id?) {
            Some(Slot::Ready(r)) => Some(r),
            _ => None,
        }
    }

    fn current_assets(&self) -> Option<Arc<MapAssets>> {
        let d = self.docs.get(self.active)?;
        // a tab with terrain edits sees the ground as edited
        if let (Some(v), Some(p)) = (self.terr_views.get(&d.uid), d.part()) {
            if v.is_for(p) {
                return Some(v.assets.clone());
            }
        }
        self.ready_for(d.map_id).map(|r| r.assets.clone())
    }

    // ------------------------------------------------------------------------------------------------------------ validation

    fn run_validation(&mut self) {
        let now = Instant::now();
        for i in 0..self.docs.len() {
            if !self.docs[i].validate_due.is_some_and(|t| t <= now) {
                continue;
            }
            let (mut f, map_id) = {
                let d = &self.docs[i];
                let mut f = validate(&d.map);
                if let Some(folder) = &self.folder {
                    let own = d.map_id;
                    let lookup = |id: u32| -> Option<&Svmap> {
                        if Some(id) == own {
                            Some(&d.map)
                        } else {
                            folder.maps.get(&id).and_then(|m| m.svmap.as_deref())
                        }
                    };
                    // maps still loading are not "missing": skip the cross rules until the folder is read
                    if folder.rx.is_none() {
                        f.extend(validate_cross(&d.map, &lookup));
                    }
                }
                if let Some(c) = &self.client {
                    f.extend(c.check_with(d.map_id, &d.map, d.part().map(|p| p.wld_arc())));
                }
                (f, d.map_id)
            };
            let _ = map_id;
            f.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.section.cmp(&b.section)).then(a.index.cmp(&b.index)));
            self.docs[i].findings = f;
            self.docs[i].validate_due = None;
        }
    }

    // ------------------------------------------------------------------------------------------------------------ editing helpers

    fn ground_fn(&self) -> impl Fn(f32, f32) -> f32 {
        let a = self.current_assets();
        move |x, z| a.as_ref().map_or(0.0, |a| a.ground_at(x, z))
    }

    fn drop_selection_to_ground(&mut self) {
        let g = self.ground_fn();
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let mut ops = Vec::new();
        for (s, i) in d.selection.clone() {
            if let Some(before) = edit::get(&d.map, s, i) {
                let mut after = before.clone();
                edit::drop_to_ground(&mut after, &g);
                if after != before {
                    ops.push(Op::replace(i, before, after));
                }
            }
        }
        if !ops.is_empty() {
            d.commit("drop to ground", ops);
        }
    }

    fn nudge(&mut self, dx: f32, dz: f32) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let mut ops = Vec::new();
        for (s, i) in d.selection.clone() {
            if let Some(before) = edit::get(&d.map, s, i) {
                let mut after = before.clone();
                edit::translate(&mut after, dx, dz);
                ops.push(Op::replace(i, before, after));
            }
        }
        if !ops.is_empty() {
            d.commit("nudge", ops);
        }
    }

    fn mismatch_text(&self, what: &Pending) -> (String, String) {
        let Some(d) = self.docs.get(self.active) else { return (String::new(), String::new()) };
        let a = self.current_assets();
        let wld = a.as_ref().and_then(|a| a.wld.clone());
        let id = d.map_id.map_or("?".to_string(), |i| i.to_string());
        let count_line = |sec: &str, now: usize, after: usize, wld_n: Option<usize>| match wld_n {
            Some(w) => format!("The client's world/{id}.wld has {w} {sec}; this map has {now} now and would have {after}. "),
            None => format!("No client data is open. The match with world/{id}.wld cannot be checked. This map has {now} {sec} and would have {after}. "),
        };
        let wp = d.part().map(|p| p.count(SectionId::Portals)).or(wld.as_ref().map(|w| w.portals.len()));
        let wr = d.part().map(|p| p.count(SectionId::Regions)).or(wld.as_ref().map(|w| w.areas.len()));
        match what {
            Pending::AddPortal(_) => (
                "Add a portal".to_string(),
                count_line("portals", d.map.portals.len(), d.map.portals.len() + 1, wp) + "The client sends the portal index, so the .wld needs a matching portal box at the same index. Problems shows portal-count until it has one.",
            ),
            Pending::Paste(recs, _) => {
                let np = recs.iter().filter(|r| matches!(r, Rec::Portal(_))).count();
                let nr = recs.iter().filter(|r| matches!(r, Rec::Region(_))).count();
                let mut t = String::new();
                if np > 0 {
                    t += &count_line("portals", d.map.portals.len(), d.map.portals.len() + np, wp);
                }
                if nr > 0 {
                    t += &count_line("areas", d.map.regions.len(), d.map.regions.len() + nr, wr);
                }
                ("Paste records".to_string(), t + "The count must match the client's list.")
            }
            Pending::Delete => {
                let np = d.selection.iter().filter(|(s, _)| *s == SectionId::Portals).count();
                let nr = d.selection.iter().filter(|(s, _)| *s == SectionId::Regions).count();
                let mut t = String::new();
                if np > 0 {
                    t += &count_line("portals", d.map.portals.len(), d.map.portals.len() - np.min(d.map.portals.len()), wp);
                    t += "Portals behind a removed one shift their index. The client sends the index. ";
                }
                if nr > 0 {
                    t += &count_line("areas", d.map.regions.len(), d.map.regions.len() - nr.min(d.map.regions.len()), wr);
                }
                ("Delete records".to_string(), t)
            }
            Pending::MovePortal(i, up) => {
                let j = if *up { i.saturating_sub(1) } else { i + 1 };
                ("Reorder portals".to_string(), format!("Swaps portals #{i} and #{j}. The client sends the portal index and its world/{id}.wld keeps its own order. Both portals differ from the .wld (portal-field) until the order matches."))
            }
            Pending::Revert { ops, label } => {
                let word = if label.starts_with("take") { "take" } else { "revert" };
                let sec = |o: &Op| matches!(o.section(), SectionId::Portals | SectionId::Regions);
                let ins = ops.iter().filter(|o| matches!(o.kind, edit::OpKind::Insert(_)) && sec(o)).count();
                let rem = ops.iter().filter(|o| matches!(o.kind, edit::OpKind::Remove(_)) && sec(o)).count();
                (
                    format!("{} changes portals or regions", if word == "take" { "Take" } else { "Revert" }),
                    format!("This {word} adds {ins} and removes {rem} portal or region records. The client sends the portal index and mirrors the region list. Count and order must match world/{id}.wld. Problems shows portal-count and region-count where they do not."),
                )
            }
            Pending::FixAll { code, count, .. } => ("Fix all".to_string(), format!("Fix {count} finding{} of {code} on this map? It is one undo step.", if *count == 1 { "" } else { "s" })),
        }
    }

    /// Runs `action` now, or asks first when it changes a list whose order/count the client mirrors.
    fn request(&mut self, action: Pending) {
        // every pending action edits the active tab: a read-only tab refuses before any confirmation is asked
        if let Some(d) = self.docs.get_mut(self.active).filter(|d| d.read_only) {
            d.refused += 1;
            return;
        }
        // a paired section changes the client list at the same index, so only an unpaired one asks
        let follows = |s: SectionId| self.docs.get(self.active).is_some_and(|d| d.part().is_some_and(|p| pair::slot(s).is_some_and(|k| p.paired(&d.map)[k])));
        let needs_confirm = match &action {
            Pending::AddPortal(_) | Pending::MovePortal(..) => !follows(SectionId::Portals),
            Pending::Delete => self.docs.get(self.active).is_some_and(|d| d.selection.iter().any(|(s, _)| matches!(s, SectionId::Portals | SectionId::Regions) && !follows(*s))),
            Pending::Paste(r, _) => r.iter().any(|r| matches!(r, Rec::Portal(_) | Rec::Region(_)) && !follows(r.section())),
            Pending::Revert { ops, .. } => crate::compare::changes_portal_or_region_count(ops) && ops.iter().any(|o| matches!(o.section(), SectionId::Portals | SectionId::Regions) && !follows(o.section())),
            Pending::FixAll { .. } => true,
        };
        if needs_confirm {
            let (title, text) = self.mismatch_text(&action);
            let uid = self.docs[self.active].uid;
            self.dialog = Some(Dialog::Confirm { uid, title, text, action });
        } else {
            self.apply(action);
        }
    }

    fn apply(&mut self, action: Pending) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        match action {
            Pending::AddPortal(pos) => {
                let idx = d.map.portals.len();
                let to_map = d.map_id.unwrap_or(0);
                let p = shaiya_svmap::Portal { pos, country: 0, level_min: 0, level_max: 0, to_map, to_pos: pos };
                d.commit("add portal", vec![Op::insert(idx, Rec::Portal(p))]);
                d.select_only(SectionId::Portals, idx);
            }
            Pending::Delete => {
                d.delete_selected();
            }
            Pending::Revert { ops, label } | Pending::FixAll { ops, label, .. } => d.commit(label, ops),
            Pending::MovePortal(i, up) => {
                let j = if up { i.checked_sub(1) } else { Some(i + 1) };
                if let Some(j) = j.filter(|j| *j < d.map.portals.len() && i < d.map.portals.len()) {
                    let (a, b) = (d.map.portals[i], d.map.portals[j]);
                    d.commit("reorder portals", vec![Op::replace(i, Rec::Portal(a), Rec::Portal(b)), Op::replace(j, Rec::Portal(b), Rec::Portal(a))]);
                    d.select_only(SectionId::Portals, j);
                }
            }
            Pending::Paste(recs, at) => {
                if recs.is_empty() {
                    return;
                }
                // move the group so the first record's lower-left lands at the cursor (or +4 units when there is none)
                let first = edit::bounds_xz(&recs[0]);
                let (dx, dz) = match at {
                    Place::Cursor(p) => (p[0] - first.0, p[1] - first.1),
                    Place::Offset => (4.0, 4.0),
                    Place::Source => (0.0, 0.0),
                };
                let (dx, dz) = (view2d::snap_to(dx, 0.1), view2d::snap_to(dz, 0.1));
                let mut counts: std::collections::HashMap<SectionId, usize> = std::collections::HashMap::new();
                let mut ops = Vec::new();
                let mut sel = Vec::new();
                for mut r in recs {
                    edit::translate(&mut r, dx, dz);
                    let s = r.section();
                    let n = counts.entry(s).or_insert_with(|| edit::count(&d.map, s));
                    ops.push(Op::insert(*n, r));
                    sel.push((s, *n));
                    *n += 1;
                }
                let label = if ops.len() == 1 { "paste record".to_string() } else { format!("paste {} records", ops.len()) };
                // a refused commit (read-only tab) must not leave a selection of records that do not exist
                let applies = !d.read_only;
                d.commit(label, ops);
                if applies {
                    d.selection = sel;
                    d.sub = None;
                }
            }
        }
    }

    /// Paste from the app's clipboard: at the cursor, or exactly where the records were (`in_place`).
    fn paste(&mut self, in_place: bool) {
        if self.clipboard.is_empty() || self.docs.is_empty() {
            return;
        }
        let at = match (in_place, self.vp.hover) {
            (true, _) => Place::Source,
            (false, Some(p)) => Place::Cursor(p),
            (false, None) => Place::Offset,
        };
        let recs = self.clipboard.clone();
        self.request(Pending::Paste(recs, at));
    }

    fn duplicate(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let mut sel = d.selection.clone();
        sel.sort();
        let recs: Vec<Rec> = sel.iter().filter_map(|(s, i)| edit::get(&d.map, *s, *i)).collect();
        if !recs.is_empty() {
            self.request(Pending::Paste(recs, Place::Offset));
        }
    }

    fn delete(&mut self) {
        if self.docs.get(self.active).is_some_and(|d| !d.selection.is_empty()) {
            self.request(Pending::Delete);
        }
    }

    fn frame_selection(&mut self) {
        if self.mode3d {
            self.run_cmd3d(crate::view3d::Cmd3d::FrameSelection);
            return;
        }
        let size = self.vp.size;
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (s, i) in &d.selection {
            if let Some(r) = edit::get(&d.map, *s, *i) {
                let t = edit::bounds_xz(&r);
                b = (b.0.min(t.0), b.1.min(t.1), b.2.max(t.2), b.3.max(t.3));
            }
        }
        if b.0 <= b.2 {
            view2d::frame_box(d, size, b);
        }
    }

    fn frame_map(&mut self) {
        if self.mode3d {
            self.run_cmd3d(crate::view3d::Cmd3d::FrameMap);
            return;
        }
        let size = self.vp.size;
        if let Some(d) = self.docs.get_mut(self.active) {
            let s = d.map.size as f32;
            view2d::fit_view(d, size, s);
        }
    }

    fn select_finding(&mut self, f: &Finding) {
        let size = self.vp.size;
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if let Some(i) = f.index {
            if i < edit::count(&d.map, f.section) {
                d.select_only(f.section, i);
                d.sub = f.sub.filter(|_| matches!(f.section, SectionId::Npcs | SectionId::Binds));
                if let Some(r) = edit::get(&d.map, f.section, i) {
                    view2d::frame_box(d, size, edit::bounds_xz(&r));
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------------------ saving

    fn request_save(&mut self, as_new: bool) {
        let Some(d) = self.docs.get(self.active) else { return };
        let uid = d.uid;
        let target = if as_new || d.path.is_none() {
            let mut dlg = rfd::FileDialog::new().add_filter("svmap", &["svmap"]);
            if let Some(p) = &d.path {
                if let Some(dir) = p.parent() {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(n) = p.file_name() {
                    dlg = dlg.set_file_name(n.to_string_lossy());
                }
            } else if let Some(id) = d.map_id {
                dlg = dlg.set_file_name(format!("{id}.svmap"));
                if let Some(dir) = self.settings.svmap_folder.as_ref().filter(|p| Path::new(p).is_dir()) {
                    dlg = dlg.set_directory(dir);
                }
            }
            match dlg.save_file() {
                Some(p) => Some(p),
                None => return,
            }
        } else {
            None
        };
        self.begin_save(uid, target, false);
    }

    fn begin_save(&mut self, uid: u64, target: Option<PathBuf>, skip_checks: bool) {
        self.begin_save_ext(uid, target, skip_checks, false);
    }

    /// Saves a tab: the checks first (errors, a file that changed on disk), then the backup of the file about to be replaced,
    /// then the safe save. `skip_backup` is the answer to a failed backup ("save without one").
    fn begin_save_ext(&mut self, uid: u64, target: Option<PathBuf>, skip_checks: bool, skip_backup: bool) {
        let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { return };
        if self.docs[i].read_only {
            self.toast("read-only tab: it cannot be saved");
            return;
        }
        // only the client half changed: the svmap file is not written again
        let only_client = {
            let d = &self.docs[i];
            target.is_none() && d.path.is_some() && !d.svmap_dirty() && d.client_dirty()
        };
        if only_client {
            let uid = self.docs[i].uid;
            if self.save_client_part(uid) {
                self.recovery_remove(uid);
            }
            return;
        }
        if !skip_checks {
            let d = &self.docs[i];
            let errors: Vec<String> = d.findings.iter().filter(|f| f.severity == Severity::Error).map(|f| format!("{} #{}: {}", f.section.label(), f.index.map_or("-".to_string(), |x| x.to_string()), f.message)).collect();
            let disk_changed = target.is_none() && d.changed_on_disk();
            if !errors.is_empty() || disk_changed {
                self.dialog = Some(Dialog::SaveCheck { uid, target, errors, disk_changed });
                return;
            }
        }
        if !skip_backup {
            match self.backup_before_save(i, target.as_deref()) {
                Ok(Some(p)) => log::info(format!("backup {}", p.display())),
                Ok(None) => {}
                Err(why) => {
                    log::error(format!("backup failed: {why}"));
                    self.dialog = Some(Dialog::BackupFailed { uid, target, why });
                    return;
                }
            }
        }
        let d = &mut self.docs[i];
        match d.save(target.as_deref()) {
            Ok(p) => {
                let id = d.map_id;
                let copy = Arc::new(d.map.clone());
                self.settings.add_recent(&p.display().to_string());
                if let (Some(f), Some(id)) = (&mut self.folder, id) {
                    match f.maps.get_mut(&id) {
                        Some(e) => {
                            if e.path == p {
                                e.svmap = Some(copy);
                            }
                        }
                        // a new map saved into the open folder joins the map list
                        None => {
                            if f.maps.values().next().and_then(|m| m.path.parent()) == p.parent() {
                                f.maps.insert(id, FolderMap { path: p.clone(), svmap: Some(copy) });
                            }
                        }
                    }
                }
                log::info(format!("saved and verified {}", p.display()));
                self.toast(format!("saved and verified {}", p.display()));
                // the client half, when it has unsaved changes: a failure leaves it modified and the svmap saved
                if self.save_client_part(uid) {
                    self.recovery_remove(uid);
                }
            }
            Err(e) => {
                log::error(format!("save failed: {e}"));
                self.cancel_save_all();
                self.dialog = Some(Dialog::Error(e));
            }
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.docs.len() {
            return;
        }
        let uid = self.docs[i].uid;
        self.blocked_tex.remove(&uid);
        self.v3.forget(uid);
        // a tab that is closed is saved or discarded: its recovery entry goes
        self.recovery_remove(uid);
        self.safety.forget(uid);
        self.docs.remove(i);
        if self.active >= self.docs.len() && !self.docs.is_empty() {
            self.active = self.docs.len() - 1;
        }
        if self.pick_dest.as_ref().is_some_and(|p| p.src_uid == uid) {
            self.pick_dest = None;
        }
        self.vp = VpState::default();
    }

    fn try_close_tab(&mut self, i: usize) {
        if i >= self.docs.len() {
            return;
        }
        if self.docs[i].is_dirty() {
            let uid = self.docs[i].uid;
            self.active = i;
            self.dialog = Some(Dialog::Unsaved { uid, then: Then::CloseTab });
        } else {
            self.close_tab(i);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ keys

    /// A modal window is open: the keys are left alone.
    fn modal_open(&self) -> bool {
        self.dialog.is_some() || self.wizard.is_some() || self.safety.recovery.is_some() || self.q3.prompt.is_some() || self.r1.save_prompt.is_some()
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        // Shift+F in the 3D view is the fly mode, not "frame selection"
        let shift_f = self.mode3d && !typing && ctx.input(|i| i.key_pressed(Key::F) && i.modifiers.shift && !i.modifiers.command);
        let fly = crate::view3d::key_command(Key::F, true);
        let mut pressed = ctx.input_mut(|i| commands::take_pressed(i, typing));
        if shift_f {
            pressed.retain(|c| *c != Cmd::FrameSelection);
        }
        if self.modal_open() {
            self.tab_pressed = false;
            return;
        }
        // Ctrl+C and Ctrl+V reach the app as clipboard events, not as key presses (egui-winit turns them into `Event::Copy` and
        // `Event::Paste(text)` and sends no key event); a text field that has the focus takes them itself. A key press that did
        // arrive (another backend) still works, and one press never runs twice.
        let (copy_ev, paste_ev) = if typing {
            (false, None)
        } else {
            ctx.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::Copy)), i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None })))
        };
        if copy_ev {
            pressed.retain(|c| *c != Cmd::Copy);
            pressed.push(Cmd::Copy);
        }
        if let Some(text) = paste_ev {
            pressed.retain(|c| *c != Cmd::Paste);
            self.paste_text(&text);
        }
        if let (true, Some(cmd)) = (shift_f, fly) {
            self.run_cmd3d(cmd);
        }
        if std::mem::take(&mut self.tab_pressed) && !self.docs.is_empty() {
            self.set_mode3d(!self.mode3d);
        }
        for c in pressed {
            self.run(c, ctx);
        }
        if typing {
            return;
        }
        // the keys that are not commands: N (3D), arrows, Esc, the brush keys
        let n_key = ctx.input(|i| i.key_pressed(Key::N));
        let (esc, arrows, shift) = ctx.input(|i| {
            let a = [(Key::ArrowLeft, -1.0, 0.0), (Key::ArrowRight, 1.0, 0.0), (Key::ArrowUp, 0.0, 1.0), (Key::ArrowDown, 0.0, -1.0)]
                .iter()
                .filter(|(k, _, _)| i.key_pressed(*k))
                .map(|(_, x, z)| (*x, *z))
                .collect::<Vec<(f32, f32)>>();
            (i.key_pressed(Key::Escape), a, i.modifiers.shift)
        });
        if let (true, Some(cmd)) = (n_key && self.mode3d, crate::view3d::key_command(Key::N, false)) {
            self.run_cmd3d(cmd);
        }
        if esc {
            if self.shortcuts_open {
                self.shortcuts_open = false;
            } else if self.pick_dest.is_none() && self.c5.pick.is_none() {
                if self.tool == Tool::Select && self.r1.vo.active() {
                    // viewport only: Esc brings the panels back when no tool action is pending
                    let l = &mut self.settings.layout;
                    self.r1.vo.toggle(l);
                }
                if self.tool == Tool::Scatter && self.vp.scatter.active() {
                    // Esc cancels the region first; a second Esc leaves the tool
                    self.vp.scatter.clear();
                } else if self.tool == Tool::Measure && self.clear_measure() {
                    // Esc clears the measurement first; a second Esc leaves the tool
                } else {
                    self.tool = Tool::Select;
                }
            }
        }
        if self.tool == Tool::Scatter && !self.mode3d && self.r5.showing == Showing::Map {
            self.scatter_keys(ctx);
        }
        if self.tool == Tool::Paint {
            let (down, up) = ctx.input(|i| (i.key_pressed(Key::OpenBracket), i.key_pressed(Key::CloseBracket)));
            if down {
                self.paint.radius = (self.paint.radius - 1.0).max(0.0);
            }
            if up {
                self.paint.radius = (self.paint.radius + 1.0).min(view2d::MAX_BRUSH);
            }
        }
        let step = self.settings.snap * if shift { 10.0 } else { 1.0 };
        let on_map = self.r5.showing == Showing::Map;
        for (x, z) in arrows.into_iter().filter(|_| on_map) {
            self.nudge(x * step, z * step);
        }
    }

    /// Why a command cannot run right now (the reason is the disabled menu item's tooltip), or Ok.
    pub(crate) fn can_run(&self, cmd: Cmd) -> Result<(), &'static str> {
        // while the World or Compare tab shows, the hidden map is not edited from the keyboard or the menus
        let doc = self.docs.get(self.active).filter(|_| self.r5.showing == Showing::Map);
        let hidden = self.r5.showing != Showing::Map && !self.docs.is_empty();
        let need_doc = || if doc.is_some() { Ok(()) } else if hidden { Err("a map tab is not showing") } else { Err("no map open") };
        let need_selection = || match doc {
            None => Err("no map open"),
            Some(d) if d.selection.is_empty() => Err("nothing selected"),
            Some(_) => Ok(()),
        };
        match cmd {
            Cmd::ShowBrowser | Cmd::ShowInspector | Cmd::ShowDock | Cmd::ViewportOnly | Cmd::LayoutSimple | Cmd::LayoutEditing | Cmd::LayoutReview | Cmd::SaveLayout | Cmd::ManageLayouts | Cmd::OpenFile | Cmd::OpenFolder | Cmd::OpenClient | Cmd::QuickOpen | Cmd::Exit | Cmd::Preferences | Cmd::GoTo | Cmd::Palette | Cmd::Help | Cmd::Shortcuts | Cmd::Setup | Cmd::Tool(_) => Ok(()),
            Cmd::HeatOff | Cmd::HeatDensity | Cmd::HeatCycle => need_doc(),
            Cmd::HeatLevel => {
                need_doc()?;
                if self.have_levels() { Ok(()) } else { Err("needs the client data (File > Open client folder...)") }
            }
            Cmd::Rulers => Ok(()),
            Cmd::SpawnCheck => if self.docs.is_empty() && self.folder.is_none() { Err("no map or map folder is open") } else { Ok(()) },
            Cmd::BulkEdit => if self.docs.is_empty() && self.folder.is_none() { Err("no map or map folder is open") } else { Ok(()) },
            Cmd::ReloadClient => if self.client_path.is_some() { Ok(()) } else { Err("no client folder is open") },
            Cmd::NewFromClient => if self.client.is_some() { Ok(()) } else { Err("needs the client data (File > Open client folder...)") },
            Cmd::CloseTab | Cmd::FrameMap | Cmd::Toggle3d | Cmd::ValidateNow | Cmd::GenerateBlocked | Cmd::FocusOutliner | Cmd::CopyPosition => need_doc(),
            Cmd::Save | Cmd::SaveAs => {
                need_doc()?;
                if doc.is_some_and(|d| d.read_only) { Err("read-only tab: use Copy, then paste into an editable tab") } else { Ok(()) }
            }
            Cmd::SaveAll => if self.docs.iter().any(|d| d.is_dirty() && !d.read_only) { Ok(()) } else { Err("no map has unsaved changes") },
            Cmd::RestoreBackup => {
                if self.data_dir.is_none() {
                    return Err("this run has no data folder (--no-settings)");
                }
                if doc.is_some_and(|d| d.path.is_some() && d.map_id.is_some()) { Ok(()) } else { Err("needs an open map that is saved in a file") }
            }
            Cmd::CopyGmMove => if doc.is_some_and(|d| d.map_id.is_some()) { Ok(()) } else { Err("needs an open map that has a number") },
            Cmd::NextTab | Cmd::PrevTab => if self.docs.len() > 1 { Ok(()) } else { Err("only one map is open") },
            Cmd::Undo => if doc.is_some_and(|d| d.can_undo()) { Ok(()) } else { Err("nothing to undo") },
            Cmd::Redo => if doc.is_some_and(|d| d.can_redo()) { Ok(()) } else { Err("nothing to redo") },
            Cmd::Copy if self.measure_text().is_some() => Ok(()),
            Cmd::Duplicate | Cmd::Delete if self.tool == Tool::Place => if doc.is_some_and(|d| !d.place_sel.is_empty()) { Ok(()) } else { Err("no placement selected") },
            Cmd::Copy | Cmd::Duplicate | Cmd::Delete | Cmd::DropToGround | Cmd::FrameSelection | Cmd::SelectSimilar => need_selection(),
            Cmd::Paste | Cmd::PasteInPlace => {
                need_doc()?;
                if self.clipboard.is_empty() { Err("nothing copied yet (Ctrl+V also pastes records copied in another editor window)") } else { Ok(()) }
            }
            Cmd::SelectAll | Cmd::SelectMatching | Cmd::AddBookmark | Cmd::ExportAreasCsv | Cmd::ExportNpcsCsv | Cmd::ExportPortalsCsv | Cmd::ExportBindsCsv | Cmd::ExportRegionsCsv | Cmd::ExportAllCsv | Cmd::ExportJson => need_doc(),
            Cmd::ExportPng => {
                need_doc()?;
                if self.mode3d { Err("switch to the 2D view: the picture is the 2D viewport") } else { Ok(()) }
            }
            Cmd::FindInMaps => if self.docs.is_empty() && self.folder.is_none() { Err("no map or map folder is open") } else { Ok(()) },
            Cmd::WorldView | Cmd::CompareFolders => if self.folder.is_some() { Ok(()) } else { Err("needs the open map folder (File > Open folder...)") },
            Cmd::PortalLinks | Cmd::ExportFolderAreasCsv => if self.folder.is_some() { Ok(()) } else { Err("needs the open map folder (File > Open folder...)") },
            Cmd::SyncFromClient => {
                need_doc()?;
                if self.client.is_some() { Ok(()) } else { Err("needs the client data (File > Open client folder...)") }
            }
            Cmd::MapIniEditor => if self.ini_dir().is_some() { Ok(()) } else { Err("needs the server data folder with Map.ini (Edit > Preferences > Folders)") },
            Cmd::AddMap => {
                if self.client.is_none() {
                    return Err("needs the client data (File > Open client folder...)");
                }
                if self.ini_dir().is_none() {
                    return Err("needs the server data folder with Map.ini (Edit > Preferences > Folders)");
                }
                if self.folder.is_some() { Ok(()) } else { Err("needs the open map folder (File > Open folder...)") }
            }
            Cmd::ShareClient => if self.client_path.is_some() { Ok(()) } else { Err("needs the client data (File > Open client folder...)") },
            Cmd::StageClientMap => {
                need_doc()?;
                if self.client.is_none() {
                    return Err("needs the client data (File > Open client folder...)");
                }
                if doc.is_some_and(|d| d.map_id.is_some()) { Ok(()) } else { Err("needs an open map that has a number") }
            }
            Cmd::SyncSelected => {
                if self.client.is_none() {
                    return Err("needs the client data (File > Open client folder...)");
                }
                if doc.is_some_and(|d| d.single().is_some_and(|(s, _)| tools_sync_sections(s))) { Ok(()) } else { Err("select one NPC, portal, region or bind area") }
            }
            Cmd::OpenDataFolder => if self.data_dir.is_some() { Ok(()) } else { Err("this run has no data folder (--no-settings)") },
        }
    }

    /// Runs a command (from a menu, a key, the palette or a Help link). A command that cannot run now does nothing.
    pub(crate) fn run(&mut self, cmd: Cmd, ctx: &egui::Context) {
        if self.can_run(cmd).is_err() {
            return;
        }
        match cmd {
            Cmd::OpenFile => self.menu_open_file(),
            Cmd::OpenFolder => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.open_folder(&p, ctx);
                }
            }
            Cmd::OpenClient => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.open_client(&p, ctx);
                }
            }
            Cmd::ReloadClient => self.reload_client(ctx),
            Cmd::QuickOpen => self.palette = Some(Palette::new(PalMode::Maps)),
            Cmd::NewFromClient => self.open_new_from_client(),
            Cmd::Save => self.request_save(false),
            Cmd::SaveAs => self.request_save(true),
            Cmd::SaveAll => self.start_save_all(),
            Cmd::RestoreBackup => self.open_backups_window(),
            Cmd::CloseTab => {
                let a = self.active;
                self.try_close_tab(a);
            }
            Cmd::NextTab => {
                let n = self.docs.len();
                self.switch_to((self.active + 1) % n);
            }
            Cmd::PrevTab => {
                let n = self.docs.len();
                self.switch_to((self.active + n - 1) % n);
            }
            Cmd::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Cmd::Undo => {
                if let Some(d) = self.docs.get_mut(self.active) {
                    d.undo();
                }
            }
            Cmd::Redo => {
                if let Some(d) = self.docs.get_mut(self.active) {
                    d.redo();
                }
            }
            Cmd::Copy => match self.measure_text() {
                Some(t) => {
                    ctx.copy_text(t.clone());
                    self.toast(format!("Copied: {t}"));
                }
                None => self.copy_selection_to(ctx),
            },
            Cmd::Paste => self.paste(false),
            Cmd::PasteInPlace => self.paste(true),
            Cmd::SelectAll => self.select_all(),
            Cmd::SelectSimilar => self.select_similar(),
            Cmd::SelectMatching => self.show_find(true),
            Cmd::FindInMaps => self.show_find(false),
            Cmd::AddBookmark => self.add_bookmark_at(None),
            Cmd::PortalLinks => self.q3.links_open = true,
            Cmd::ExportAreasCsv | Cmd::ExportNpcsCsv | Cmd::ExportPortalsCsv | Cmd::ExportBindsCsv | Cmd::ExportRegionsCsv | Cmd::ExportAllCsv | Cmd::ExportFolderAreasCsv | Cmd::ExportJson | Cmd::ExportPng => self.run_export(cmd),
            Cmd::Duplicate => if self.tool == Tool::Place { self.place_duplicate() } else { self.duplicate() },
            Cmd::Delete => if self.tool == Tool::Place { self.place_delete() } else { self.delete() },
            Cmd::DropToGround => self.drop_selection_to_ground(),
            Cmd::FocusOutliner => {
                self.settings.layout.show(PanelId::Browser);
                if !self.settings.layout.browser_split {
                    self.settings.layout.browser_tab = crate::layout::BrowserTab::Outliner;
                }
                self.focus_outliner = true;
            }
            Cmd::ShowBrowser | Cmd::ShowInspector | Cmd::ShowDock | Cmd::ViewportOnly | Cmd::LayoutSimple | Cmd::LayoutEditing | Cmd::LayoutReview | Cmd::SaveLayout | Cmd::ManageLayouts => self.run_layout_cmd(cmd),
            Cmd::Preferences => self.prefs = Some(Prefs::new(&self.settings)),
            Cmd::HeatCycle => self.cycle_heat(),
            Cmd::HeatOff => self.set_heat_mode(crate::heat::Mode::Off),
            Cmd::HeatDensity => self.set_heat_mode(crate::heat::Mode::Density),
            Cmd::HeatLevel => self.set_heat_mode(crate::heat::Mode::Level),
            Cmd::Rulers => self.settings.rulers = !self.settings.rulers,
            Cmd::SpawnCheck => self.spawn_check(),
            Cmd::BulkEdit => self.open_bulk(None),
            Cmd::WorldView => self.open_world(),
            Cmd::CompareFolders => self.open_compare_folders(),
            Cmd::FrameSelection => self.frame_selection(),
            Cmd::FrameMap => self.frame_map(),
            Cmd::Toggle3d => self.set_mode3d(!self.mode3d),
            Cmd::GoTo => self.palette = Some(Palette::new(PalMode::GoTo)),
            Cmd::Palette => self.palette = Some(Palette::new(PalMode::Commands)),
            Cmd::CopyPosition => self.copy_position(ctx, false),
            Cmd::CopyGmMove => self.copy_position(ctx, true),
            Cmd::Tool(t) => self.tool = t,
            Cmd::ValidateNow => self.mark_all_for_validation(),
            Cmd::GenerateBlocked => self.open_generate(None),
            Cmd::SyncFromClient => self.open_sync(),
            Cmd::SyncSelected => self.sync_selected_record(),
            Cmd::StageClientMap => self.stage_client_map(),
            Cmd::MapIniEditor => self.open_ini_editor(),
            Cmd::AddMap => self.open_add_map(),
            Cmd::ShareClient => self.open_share(),
            Cmd::Help => self.open_help_for_state(),
            Cmd::Shortcuts => self.shortcuts_open = !self.shortcuts_open,
            Cmd::Setup => self.open_setup(),
            Cmd::OpenDataFolder => {
                if let Some(d) = &self.data_dir {
                    let _ = std::fs::create_dir_all(d);
                    prefs::reveal(d);
                }
            }
        }
    }

    /// F1: the guide at the page for the active tool, else for the selected section, else the start.
    fn open_help_for_state(&mut self) {
        let selected = self.docs.get(self.active).and_then(|d| d.single()).map(|(s, _)| s);
        let special = match self.r5.showing {
            Showing::World => Some("world-view"),
            Showing::Compare => Some("compare-folders"),
            Showing::Map => None,
        };
        let anchor = if let Some(a) = special {
            a
        } else if selected.is_none() && self.tool == Tool::Select && self.dock_showing(DockTab::Spawns) { "the-spawns-tab" } else { help::anchor_for(self.tool, self.draw_target, selected) };
        self.help_open = true;
        self.help.goto(anchor);
    }

    fn menu_open_file(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("svmap", &["svmap"]).pick_file() {
            self.open_file(&p);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ panels

    fn recent_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.menu_button("Recent", |ui| {
            enum Act {
                Open(String, bool),
                Pin(String),
                Clear,
            }
            let entries = self.recent_entries();
            if entries.is_empty() {
                ui.label("(none)");
                return;
            }
            let mut act = None;
            for (path, is_dir, exists, pinned) in &entries {
                ui.horizontal(|ui| {
                    if ui.small_button(if *pinned { "unpin" } else { "pin" }).on_hover_text("Pinned entries stay at the top and survive Clear unpinned").clicked() {
                        act = Some(Act::Pin(path.clone()));
                    }
                    let label = self.recent_label(path, *is_dir);
                    let label = if *exists { label } else { format!("{label}  (missing)") };
                    if ui.add_enabled(*exists, egui::Button::new(label)).clicked() {
                        act = Some(Act::Open(path.clone(), *is_dir));
                    }
                });
            }
            ui.separator();
            if ui.button("Clear unpinned").clicked() {
                act = Some(Act::Clear);
            }
            match act {
                Some(Act::Open(path, is_dir)) => {
                    ui.close();
                    let p = PathBuf::from(&path);
                    if is_dir {
                        self.open_folder(&p, ctx);
                    } else {
                        self.open_file(&p);
                    }
                }
                Some(Act::Pin(path)) => self.settings.toggle_pin(&path),
                Some(Act::Clear) => self.settings.clear_unpinned_recent(),
                None => {}
            }
        });
    }

    fn map_rows(&self) -> Vec<(u32, String, Option<u32>, Option<bool>)> {
        let f = self.map_filter.to_lowercase();
        let mut ids: Vec<u32> = match &self.folder {
            Some(fo) => fo.maps.keys().copied().collect(),
            None => self.docs.iter().filter_map(|d| d.map_id).collect(),
        };
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .filter_map(|id| {
                let name = self.map_name(id);
                if !f.is_empty() && !name.to_lowercase().contains(&f) && !id.to_string().contains(&f) {
                    return None;
                }
                let size = self.folder.as_ref().and_then(|fo| fo.maps.get(&id)).and_then(|m| m.svmap.as_ref().map(|s| s.size)).or_else(|| self.docs.iter().find(|d| d.map_id == Some(id)).map(|d| d.map.size));
                Some((id, name, size, self.kinds.get(&id).copied()))
            })
            .collect()
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        if self.docs.is_empty() {
            ui.label("No map open.");
            return;
        }
        enum Row {
            Header(SectionId),
            Rec(SectionId, usize),
            Empty(SectionId),
        }
        let assets = self.current_assets();
        let rules = self.client.clone();
        let filter = self.outliner_filter.to_lowercase();
        let names_sys = self.sysmsg.clone();
        let map_name = move |id: u32| shaiya_clientdata::map_name(id, names_sys.as_deref());
        let sections = [SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions, SectionId::SectionB, SectionId::SectionA];
        let mut rows = Vec::new();
        {
            let d = &self.docs[self.active];
            let look = Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
            for s in sections {
                let n = edit::count(&d.map, s);
                if n == 0 && filter.is_empty() {
                    // keep the header so "+" works
                }
                rows.push(Row::Header(s));
                if self.collapsed.contains(&s) {
                    continue;
                }
                if n == 0 && filter.is_empty() {
                    rows.push(Row::Empty(s));
                }
                for i in 0..n {
                    if filter.is_empty() || names::label(&look, &d.map, s, i, &map_name).to_lowercase().contains(&filter) || i.to_string() == filter {
                        rows.push(Row::Rec(s, i));
                    }
                }
            }
        }
        let mut click: Option<(SectionId, usize, bool)> = None;
        let mut hovered: Option<(SectionId, usize)> = None;
        let mut frame = false;
        let mut toggle_collapse = None;
        let mut add: Option<SectionId> = None;
        let uid_active = self.active;
        let hints = self.settings.hints;
        egui::ScrollArea::vertical().id_salt("outliner").auto_shrink([false, false]).show_rows(ui, 18.0, rows.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let d = &self.docs[uid_active];
            let look = Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
            for r in range {
                match &rows[r] {
                    Row::Header(s) => {
                        ui.horizontal(|ui| {
                            let arrow = if self.collapsed.contains(s) { ">" } else { "v" };
                            if ui.selectable_label(false, RichText::new(format!("{arrow} {} {}", s.label(), edit::count(&d.map, *s))).strong()).clicked() {
                                toggle_collapse = Some(*s);
                            }
                            // the chain of a section that is edited together with the client map
                            if let (Some(p), Some(k)) = (d.part(), pair::slot(*s)) {
                                let pal = crate::theme::pal_ui(ui);
                                if p.paired(&d.map)[k] {
                                    crate::icons::mark(ui, crate::icons::Icon::Link, pal.link, 14.0, help_text::tip("pair-linked"));
                                } else {
                                    crate::icons::mark(ui, crate::icons::Icon::Link, pal.weak.gamma_multiply(0.6), 14.0, help_text::tip("pair-unpaired"));
                                }
                            }
                            let (vis, lock) = layer_flags(&mut self.settings, *s);
                            ui.checkbox(vis, "").on_hover_text(help_text::tip("outliner-eye"));
                            ui.checkbox(lock, "lock").on_hover_text(help_text::tip("outliner-lock"));
                            if ui.small_button("+").on_hover_text(help_text::tip("outliner-add")).clicked() {
                                add = Some(*s);
                            }
                        });
                    }
                    Row::Empty(s) => {
                        let how = if hints { empty_section_hint(*s) } else { "" };
                        ui.label(RichText::new(format!("   (none) {how}")).weak());
                    }
                    Row::Rec(s, i) => {
                        let text = format!("{i:>4} {}", names::label(&look, &d.map, *s, *i, &map_name));
                        let resp = ui.selectable_label(d.is_selected(*s, *i), RichText::new(text).monospace());
                        if resp.hovered() {
                            hovered = Some((*s, *i));
                        }
                        if resp.clicked() {
                            let m = ui.input(|inp| inp.modifiers.command || inp.modifiers.shift);
                            click = Some((*s, *i, m));
                        }
                        if resp.double_clicked() {
                            frame = true;
                        }
                    }
                }
            }
        });
        if hovered.is_some() {
            self.hover_rec = hovered;
        }
        if let Some(s) = toggle_collapse {
            if !self.collapsed.remove(&s) {
                self.collapsed.insert(s);
            }
        }
        if let Some((s, i, multi)) = click {
            let d = &mut self.docs[self.active];
            if multi {
                d.toggle(s, i);
            } else {
                d.select_only(s, i);
            }
        }
        if frame {
            self.frame_selection();
        }
        if let Some(s) = add {
            self.add_at_view_centre(s);
        }
    }

    fn add_at_view_centre(&mut self, s: SectionId) {
        let g = self.ground_fn();
        let Some(d) = self.docs.get(self.active) else { return };
        // the centre of the view: the 2D view's, or the 3D camera's focus
        let (cx, cz) = match self.v3.states.get(&d.uid).filter(|_| self.mode3d) {
            Some(st) => (st.cam.focus[0], st.cam.focus[2]),
            None => (d.view.cx, d.view.cz),
        };
        let (x, z) = (view2d::snap_to(cx, self.settings.snap), view2d::snap_to(cz, self.settings.snap));
        let y = g(x, z);
        let rec = match s {
            SectionId::MobAreas => Rec::Mob(shaiya_svmap::MobArea { min: [x - 8.0, y, z - 8.0], max: [x + 8.0, y + 2.0, z + 8.0], mobs: Vec::new() }),
            SectionId::Npcs => Rec::Npc(shaiya_svmap::NpcEntry { kind: self.npc_template.0, type_id: self.npc_template.1, points: vec![shaiya_svmap::NpcPoint { pos: [x, y, z], dir: 0.0 }] }),
            SectionId::Portals => {
                self.request(Pending::AddPortal([x, y, z]));
                return;
            }
            SectionId::Binds => Rec::Bind(shaiya_svmap::Bind { country: 0, attr: 0, boxes: vec![shaiya_svmap::Aabb { min: [x - 8.0, y, z - 8.0], max: [x + 8.0, y + 2.0, z + 8.0] }] }),
            SectionId::Regions => Rec::Region(shaiya_svmap::Region { min: [x - 8.0, y, z - 8.0], max: [x + 8.0, y + 2.0, z + 8.0], kind: 0, b: 0, c: 0, d: 0 }),
            SectionId::SectionB => Rec::B([x, y, z]),
            SectionId::SectionA => Rec::A(shaiya_svmap::RawA { pos: [x, y, z], a: 0, b: 0 }),
            _ => return,
        };
        let d = &mut self.docs[self.active];
        let idx = edit::count(&d.map, s);
        d.commit(format!("add {}", s.label()), vec![Op::insert(idx, rec)]);
        d.select_only(s, idx);
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        if self.r5.showing == Showing::World {
            self.world_card(ui);
            return;
        }
        if self.r5.showing == Showing::Compare {
            ui.label("Pick a row in Compare to open its map.");
            return;
        }
        if self.docs.is_empty() {
            return;
        }
        if self.tool == Tool::Place {
            self.place_inspector(ui);
            return;
        }
        if self.tool == Tool::Terrain {
            self.terrain_inspector(ui);
            return;
        }
        let assets = self.current_assets();
        let rules = self.client.clone();
        let maps: Vec<(u32, String)> = match &self.folder {
            Some(f) => f.maps.keys().map(|id| (*id, self.map_name(*id))).collect(),
            None => Vec::new(),
        };
        let sys = self.sysmsg.clone();
        let map_name = move |id: u32| shaiya_clientdata::map_name(id, sys.as_deref());
        let active = self.active;
        // the findings of the selected record with the titles of their fixes (the inspector shows them with Fix buttons)
        let env = self.fix_env();
        let rec_fixes: Vec<(shaiya_svmap::Finding, Vec<String>)> = {
            let d = &self.docs[active];
            match d.single() {
                Some((s, i)) => {
                    let cx = env.ctx(&d.map, d.map_id);
                    d.findings.iter().filter(|f| f.section == s && f.index == Some(i)).map(|f| (f.clone(), crate::fixes::fixes_for(f, &cx).into_iter().map(|x| x.title).collect())).collect()
                }
                None => Vec::new(),
            }
        };
        let incoming = if self.docs[active].selection.is_empty() { self.incoming_portals() } else { Vec::new() };
        let sections = std::cell::RefCell::new(std::mem::take(&mut self.settings.open_sections));
        let reqs = {
            let d = &mut self.docs[active];
            let c = InspCtx { look: Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: None }, maps: &maps, map_name: &map_name, hints: self.settings.hints, fixes: &rec_fixes, snap: self.settings.snap, incoming: &incoming, sections: &sections };
            inspector::show(ui, d, &c)
        };
        self.settings.open_sections = sections.into_inner();
        let ctx = ui.ctx().clone();
        for r in reqs {
            match r {
                InspReq::PickMob { area, entry } => self.picker = Some(Picker { uid: self.docs[active].uid, kind: PickKind::Mob { area, entry }, query: String::new() }),
                InspReq::PickNpc { npc } => self.picker = Some(Picker { uid: self.docs[active].uid, kind: PickKind::Npc { npc }, query: String::new() }),
                InspReq::PickDest { portal } => self.start_pick_dest(portal),
                InspReq::DropToGround => self.drop_selection_to_ground(),
                InspReq::MovePortal { idx, up } => self.request(Pending::MovePortal(idx, up)),
                InspReq::DeleteSelected => self.delete(),
                InspReq::Duplicate => self.duplicate(),
                InspReq::FrameSelection => self.frame_selection(),
                InspReq::Fix(f, k) => self.apply_fix(&f, k),
                InspReq::SelectSimilar => self.select_similar(),
                InspReq::GoToDest { portal } => self.go_to_dest(portal),
                InspReq::DropDestToGround { portal } => self.drop_dest_to_ground(portal, &ctx),
                InspReq::SaveTemplate { area } => self.save_template_prompt(area),
                InspReq::Toast(t) => self.toast(t),
                InspReq::OpenPortal { map, index } => self.open_hit(&crate::search::Hit { map: Some(map), section: SectionId::Portals, index, label: String::new(), what: String::new() }),
            }
        }
    }

    fn start_pick_dest(&mut self, portal: usize) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(p) = d.map.portals.get(portal) else { return };
        let dest = p.to_map;
        let src_uid = d.uid;
        if Some(dest) != d.map_id {
            let open = self.docs.iter().position(|x| x.map_id == Some(dest));
            match open {
                Some(i) => self.active = i,
                None => {
                    if self.folder.as_ref().is_some_and(|f| f.maps.contains_key(&dest)) {
                        self.open_map_id(dest);
                    } else {
                        self.toast(format!("map {dest} is not in the open folder; open its folder first"));
                        return;
                    }
                }
            }
            self.vp = VpState::default();
        }
        self.pick_dest = Some(PickDest { src_uid, portal, dest_map: dest });
        self.tool = Tool::Select;
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut switch = None;
        let mut close = None;
        let mut menu_act: Option<safety::TabAct> = None;
        let mut special: (Option<Showing>, Option<Showing>) = (None, None);
        ui.horizontal(|ui| {
            special = self.special_tabs(ui);
            for (i, d) in self.docs.iter().enumerate() {
                let ro = if d.read_only { " (read-only)" } else { "" };
                let name = match (d.map_id, &d.label) {
                    (Some(id), None) => format!("{} {}{}{ro}", id, self.map_name(id), if d.is_dirty() { " •" } else { "" }),
                    _ => format!("{}{}{ro}", d.title(), if d.is_dirty() { " •" } else { "" }),
                };
                let tip = match &d.path {
                    Some(p) => format!("{}
{}", p.display(), help_text::tip("tab-dot")),
                    None => format!("Not saved yet
{}", help_text::tip("tab-dot")),
                };
                let resp = ui.selectable_label(i == self.active && self.r5.showing == Showing::Map, name).on_hover_text(tip);
                if resp.clicked() {
                    switch = Some(i);
                }
                resp.context_menu(|ui| {
                    if let Some(a) = self.tab_menu(ui, i) {
                        menu_act = Some(a);
                        ui.close();
                    }
                });
                if ui.small_button("x").on_hover_text(help_text::tip("tab-close")).clicked() {
                    close = Some(i);
                }
                ui.separator();
            }
        });
        if let Some(w) = special.0 {
            self.r5.showing = w;
        }
        if let Some(w) = special.1 {
            self.close_special(w);
        }
        if let Some(i) = switch {
            self.r5.showing = Showing::Map;
            if i != self.active {
                self.active = i;
                self.vp = VpState::default();
            }
        }
        if let Some(i) = close {
            self.try_close_tab(i);
        }
        if let Some(a) = menu_act {
            let ctx = ui.ctx().clone();
            self.do_tab_act(a, &ctx);
        }
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        if self.r5.showing != Showing::Map {
            self.central_special(ui);
            return;
        }
        if self.docs.is_empty() {
            self.central_empty(ui);
            return;
        }
        if self.mode3d {
            self.central3d(ui);
            return;
        }
        self.tab_bar(ui);
        ui.separator();
        self.disk_bar(ui);
        let ctx = ui.ctx().clone();
        self.ensure_blocked_texture(&ctx);
        let active = self.active;
        let assets = self.current_assets();
        let rules = self.client.clone();
        let sys = self.sysmsg.clone();
        let map_name = move |id: u32| shaiya_clientdata::map_name(id, sys.as_deref());
        let (tex_terrain, tex_minimap, tex_dungeon) = match self.ready_for(self.docs[active].map_id) {
            Some(r) => (r.terrain.clone(), r.minimap.clone(), r.dungeon.clone()),
            None => (None, None, None),
        };
        // a tab with terrain edits shows its own redrawn picture
        let tex_terrain = self.terrain_picture(active).or(tex_terrain);
        let compare_tex = self.compare_texture(&ctx);
        let tool = self.effective_tool();
        self.scatter_update();
        let bulk_outline = self.bulk_outline();
        let heat = self.heat_frame(&ctx);
        let (heat_tex, heat_draw) = match heat {
            Some((t, d)) => (Some(t), Some(d)),
            None => (None, None),
        };
        let tex = Textures { terrain: tex_terrain, minimap: tex_minimap, dungeon: tex_dungeon, blocked: self.blocked_tex.get(&self.docs[active].uid).cloned(), preview: self.gen_preview_texture(self.docs[active].uid), compare: compare_tex, heat: heat_tex };
        let pick_banner = self.pick_banner_text(active);
        let marker = self.crosshair.filter(|(uid, _, _)| *uid == self.docs[active].uid).map(|(_, p, _)| p);
        // the fixes of the findings of the record under an open context menu
        let menu_fixes: Vec<(Finding, Vec<String>)> = match self.vp.menu.as_ref().and_then(|m| m.hit) {
            Some(h) => {
                let env = self.fix_env();
                let d = &self.docs[active];
                let cx = env.ctx(&d.map, d.map_id);
                d.findings.iter().filter(|f| f.section == h.sec && f.index == Some(h.idx)).map(|f| (f.clone(), crate::fixes::fixes_for(f, &cx).into_iter().map(|x| x.title).collect())).collect()
            }
            None => Vec::new(),
        };
        let template = self.active_template().cloned();
        let clean = self.png_clean();
        let hover_rec = self.hover_rec.take();
        let can_paste = !self.clipboard.is_empty();
        let level_filter = self.settings.level_filter.map(|[a, b]| (a as f32, b as f32));
        let rulers = self.settings.rulers;
        let scatter_draw = self.r3.scatter.cache.as_ref().map(|c| view2d::ScatterDraw { areas: &c.preview.areas, note: &c.note });
        let out = {
            let uid = self.docs[active].uid;
            let overlay = self.safety.cmp.get(&uid).map(|c| &c.overlay);
            let d = &mut self.docs[active];
            let mut c = Ctx {
                look: Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: None },
                doc: d,
                layers: &self.settings.layers,
                locks: &self.settings.locks,
                tex: &tex,
                tool,
                snap: self.settings.snap,
                drop_on_move: self.drop_on_move,
                draw_target: self.draw_target,
                npc_template: self.npc_template,
                paint: self.paint,
                minimap_alpha: self.settings.minimap_alpha,
                pick_banner,
                map_name: &map_name,
                marker,
                compare: overlay,
                hover_rec,
                level_filter,
                overview: self.settings.overview,
                template: template.as_ref(),
                can_paste,
                clean,
                menu_fixes: &menu_fixes,
                heat: heat_draw,
                rulers,
                scatter: scatter_draw,
                bulk_outline: &bulk_outline,
                place: &mut self.place,
                terrain: &mut self.terrain,
                codepage: Codepage::from_key(&self.settings.client_codepage).unwrap_or_default(),
            };
            view2d::show(ui, &mut c, &mut self.vp)
        };
        for r in out.requests {
            match r {
                Request::Menu(a) => self.do_ctx_act(a, &ctx),
                Request::AddPortal(p) => self.request(Pending::AddPortal(p)),
                Request::Toast(t) => self.toast(t),
                Request::CancelPick => self.cancel_picks(),
                Request::PickDone(p) => self.finish_pick(p),
            }
        }
    }

    /// The central area before a map is open: what the editor does and the way in, depending on how far the setup got.
    fn central_empty(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let have_folder = self.folder.is_some();
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            ui.heading("svmap editor");
            ui.label("Edit server map files over the client's terrain.");
            ui.add_space(10.0);
            if !have_folder {
                if self.client.is_some() {
                    ui.label("Client data loaded. Open the server map folder to start.");
                } else {
                    ui.label("Open the server map folder to start. The client folder adds names, terrain and checks.");
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() / 2.0 - 190.0).max(0.0));
                    if ui.button("Open map folder...").clicked() {
                        self.run(Cmd::OpenFolder, &ctx);
                    }
                    if ui.button("Open file...").clicked() {
                        self.run(Cmd::OpenFile, &ctx);
                    }
                    if ui.button("Run setup...").clicked() {
                        self.run(Cmd::Setup, &ctx);
                    }
                });
            } else {
                ui.label("Double-click a map, or press Ctrl+P.");
                let recent: Vec<(String, PathBuf)> = self
                    .settings
                    .recent
                    .iter()
                    .map(PathBuf::from)
                    .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svmap")))
                    .take(5)
                    .map(|p| (self.recent_label(&p.display().to_string(), false), p))
                    .collect();
                if !recent.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Recent maps").weak());
                    let mut open = None;
                    for (label, p) in &recent {
                        if ui.button(label).clicked() {
                            open = Some(p.clone());
                        }
                    }
                    if let Some(p) = open {
                        self.open_file(&p);
                    }
                }
            }
            if self.settings.hints {
                ui.add_space(14.0);
                ui.label(RichText::new("F1 opens the guide. Ctrl+Shift+P finds any command.").weak());
            }
        });
    }

    /// The banner of the 2D and 3D views while a pick is on: a portal destination or a rebirth position of Map.ini.
    fn pick_banner_text(&self, active: usize) -> Option<String> {
        let id = self.docs.get(active)?.map_id;
        if let Some(p) = self.pick_dest.as_ref().filter(|p| id == Some(p.dest_map)) {
            return Some(format!("Click the destination of portal #{} (Esc cancels)", p.portal));
        }
        self.c5.pick.as_ref().filter(|p| id == Some(p.map)).map(|p| format!("Click the {} of zone {} (Esc cancels)", p.key, p.zone))
    }

    fn cancel_picks(&mut self) {
        self.pick_dest = None;
        self.c5.pick = None;
    }

    fn finish_pick(&mut self, pos: [f32; 3]) {
        if self.c5.pick.is_some() {
            self.finish_ini_pick(pos);
        } else {
            self.finish_pick_dest(pos);
        }
    }

    fn finish_pick_dest(&mut self, pos: [f32; 3]) {
        let Some(pd) = self.pick_dest.take() else { return };
        let Some(i) = self.docs.iter().position(|d| d.uid == pd.src_uid) else { return };
        let d = &mut self.docs[i];
        if let Some(Rec::Portal(before)) = edit::get(&d.map, SectionId::Portals, pd.portal) {
            let mut after = before;
            after.to_pos = pos;
            d.commit("pick portal destination", vec![Op::replace(pd.portal, Rec::Portal(before), Rec::Portal(after))]);
            d.select_only(SectionId::Portals, pd.portal);
        }
        self.active = i;
        self.vp = VpState::default();
    }

    // ------------------------------------------------------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let mut keep: Option<Dialog> = None;
        match dialog {
            Dialog::Error(e) => {
                let mut open = true;
                egui::Modal::new(egui::Id::unique("err")).show(ctx, |ui| {
                    ui.heading("Error");
                    ui.label(&e);
                    if ui.button("OK").clicked() {
                        open = false;
                    }
                });
                if open {
                    keep = Some(Dialog::Error(e));
                }
            }
            Dialog::Unsaved { uid, then } => {
                let mut action = 0;
                let name = self.docs.iter().find(|d| d.uid == uid).map(|d| d.title()).unwrap_or_default();
                egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
                    ui.heading("Unsaved changes");
                    ui.label(format!("{name} has changes that are not saved."));
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            action = 1;
                        }
                        if ui.button("Discard").clicked() {
                            action = 2;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 3;
                        }
                    });
                });
                match action {
                    0 => keep = Some(Dialog::Unsaved { uid, then }),
                    1 => {
                        self.begin_save(uid, None, false);
                        if self.dialog.is_none() {
                            if let Some(i) = self.docs.iter().position(|d| d.uid == uid && !d.is_dirty()) {
                                match then {
                                    Then::CloseTab => self.close_tab(i),
                                }
                            }
                        }
                    }
                    2 => {
                        if let Some(i) = self.docs.iter().position(|d| d.uid == uid) {
                            match then {
                                Then::CloseTab => self.close_tab(i),
                            }
                        }
                    }
                    _ => {}
                }
            }
            Dialog::UnsavedExit { names } => {
                let mut action = 0;
                egui::Modal::new(egui::Id::unique("unsaved-exit")).show(ctx, |ui| {
                    ui.heading("Unsaved changes");
                    ui.label("These maps have changes that are not saved:");
                    for n in &names {
                        ui.label(format!("  {n}"));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Discard all and exit").clicked() {
                            action = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 2;
                        }
                    });
                });
                match action {
                    0 => keep = Some(Dialog::UnsavedExit { names }),
                    1 => {
                        self.close_confirmed = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    _ => {}
                }
            }
            Dialog::SaveCheck { uid, target, errors, disk_changed } => {
                let mut action = 0;
                egui::Modal::new(egui::Id::unique("savecheck")).show(ctx, |ui| {
                    ui.heading("Save anyway?");
                    if disk_changed {
                        ui.label("The file on disk changed since it was opened (time or size). Saving overwrites it.");
                    }
                    if !errors.is_empty() {
                        ui.label(format!("{} error(s) in the Problems list:", errors.len()));
                        for e in errors.iter().take(10) {
                            ui.label(RichText::new(e).small().color(crate::theme::pal_ui(ui).error));
                        }
                        if errors.len() > 10 {
                            ui.label(format!("... and {} more", errors.len() - 10));
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Save anyway").clicked() {
                            action = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 2;
                        }
                    });
                });
                match action {
                    0 => keep = Some(Dialog::SaveCheck { uid, target, errors, disk_changed }),
                    1 => self.begin_save(uid, target, true),
                    _ => self.cancel_save_all(),
                }
            }
            Dialog::BackupFailed { uid, target, why } => {
                let mut action = 0;
                egui::Modal::new(egui::Id::unique("backup-failed")).show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.heading("The backup failed");
                    ui.label(format!("backup failed: {why}"));
                    ui.label("The editor keeps a copy of the file it is about to replace. Save without a backup, or stop and look at the data folder first.");
                    ui.horizontal(|ui| {
                        if ui.button("Save without a backup").clicked() {
                            action = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            action = 2;
                        }
                    });
                });
                match action {
                    0 => keep = Some(Dialog::BackupFailed { uid, target, why }),
                    1 => self.begin_save_ext(uid, target, true, true),
                    _ => self.cancel_save_all(),
                }
            }
            Dialog::Confirm { uid, title, text, action } => {
                let mut a = 0;
                egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.heading(&title);
                    ui.label(&text);
                    ui.horizontal(|ui| {
                        if ui.button("Do it").clicked() {
                            a = 1;
                        }
                        if ui.button("Cancel").clicked() {
                            a = 2;
                        }
                    });
                });
                match a {
                    0 => keep = Some(Dialog::Confirm { uid, title, text, action }),
                    1 if self.docs.get(self.active).is_some_and(|d| d.uid == uid) => self.apply(action),
                    _ => {}
                }
            }
        }
        if self.dialog.is_none() {
            self.dialog = keep;
        }
    }

    fn picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut pk) = self.picker.take() else { return };
        let Some(i) = self.docs.iter().position(|d| d.uid == pk.uid) else { return };
        let rules = self.client.clone();
        let assets = self.current_assets();
        let look = Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: self.docs[i].map_id, names: self.docs[i].part().map(|p| p.names()) };
        let q = pk.query.to_lowercase();
        let mut chosen: Option<u32> = None;
        let mut open = true;
        let mut open_client = false;
        let title = match pk.kind {
            PickKind::Mob { .. } => "Pick a monster",
            PickKind::Npc { .. } => "Pick an NPC",
        };
        // candidates: (id, text)
        let list: Vec<(u32, String)> = match pk.kind {
            PickKind::Mob { .. } => look
                .rules
                .and_then(|r| r.monsters.as_ref())
                .map(|m| m.records.iter().enumerate().map(|(i, r)| (i as u32, format!("{i:>5}  {}  (lv {})", r.name.trim(), r.level))).filter(|(_, t)| q.is_empty() || t.to_lowercase().contains(&q)).collect())
                .unwrap_or_default(),
            PickKind::Npc { npc } => {
                let kind = self.docs[i].map.npcs.get(npc).map_or(0, |n| n.kind);
                look.npc_list(kind).map(|l| l.iter().enumerate().map(|(k, n)| ((k + 1) as u32, format!("{:>5}  {}", k + 1, n.name.trim()))).filter(|(_, t)| q.is_empty() || t.to_lowercase().contains(&q)).collect()).unwrap_or_default()
            }
        };
        egui::Window::new(title).collapsible(false).open(&mut open).default_size([360.0, 420.0]).show(ctx, |ui| {
            ui.add(egui::TextEdit::singleline(&mut pk.query).hint_text("search by name or id").desired_width(f32::INFINITY));
            if list.is_empty() {
                if rules.is_none() {
                    ui.label(match pk.kind {
                        PickKind::Mob { .. } => "Monster names come from the client data.",
                        PickKind::Npc { .. } => "NPC names come from the client data.",
                    });
                    if ui.button("Open client folder...").clicked() {
                        open_client = true;
                    }
                } else {
                    ui.label("Nothing matches.");
                }
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, list.len(), |ui, range| {
                for r in range {
                    if ui.selectable_label(false, RichText::new(&list[r].1).monospace()).clicked() {
                        chosen = Some(list[r].0);
                    }
                }
            });
        });
        if open_client {
            self.run(Cmd::OpenClient, ctx);
        }
        if let Some(id) = chosen {
            let d = &mut self.docs[i];
            match pk.kind {
                PickKind::Mob { area, entry } => {
                    if let Some(Rec::Mob(before)) = edit::get(&d.map, SectionId::MobAreas, area) {
                        let mut after = before.clone();
                        if let Some(e) = after.mobs.get_mut(entry) {
                            e.mob = id;
                        } else if entry == after.mobs.len() {
                            // an area with no entries (area-empty): the pick adds the first one
                            after.mobs.push(shaiya_svmap::MobEntry { mob: id, count: 1 });
                        }
                        d.commit("pick monster", vec![Op::replace(area, Rec::Mob(before), Rec::Mob(after))]);
                    }
                }
                PickKind::Npc { npc } => {
                    if let Some(Rec::Npc(before)) = edit::get(&d.map, SectionId::Npcs, npc) {
                        let mut after = before.clone();
                        after.type_id = id;
                        d.commit("pick NPC", vec![Op::replace(npc, Rec::Npc(before), Rec::Npc(after))]);
                    }
                }
            }
        } else if open {
            self.picker = Some(pk);
        }
    }

    /// The Help window and the `?` shortcut overlay.
    fn help_windows(&mut self, ctx: &egui::Context) {
        if self.help_open {
            let mut open = true;
            let mut st = std::mem::take(&mut self.help);
            let actions = help::show(ctx, &mut open, &mut st, &|c| self.can_run(c));
            self.help = st;
            self.help_open = open;
            for a in actions {
                match a {
                    help::HelpAction::Run(c) => self.run(c, ctx),
                }
            }
        }
        if self.shortcuts_open {
            let mut open = true;
            help::shortcuts_overlay(ctx, &mut open);
            self.shortcuts_open = open;
        }
    }

    /// The command palette, quick-open and Go to popup.
    fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.palette.take() else { return };
        enum Pick {
            Cmd(Cmd),
            Map(MapEntry),
        }
        let mut picks: Vec<Pick> = Vec::new();
        let mut rows: Vec<palette::Row> = Vec::new();
        let mut preview: Option<Result<String, String>> = None;
        match p.mode {
            PalMode::Commands => {
                for r in palette::rank_commands(&p.query, &|c| self.can_run(c)) {
                    let why = match r.reason {
                        Some(w) => format!("Not available now: {w}"),
                        None => { let i = commands::info(r.cmd); format!("{} ({})", i.help, i.id) }
                    };
                    rows.push(palette::Row { text: r.text.clone(), right: if r.path.is_empty() { r.keys.clone() } else if r.keys.is_empty() { r.path.to_string() } else { format!("{}  {}", r.path, r.keys) }, enabled: r.reason.is_none(), why: Some(why) });
                    picks.push(Pick::Cmd(r.cmd));
                }
            }
            PalMode::Maps => {
                let entries = self.map_entries();
                for i in palette::rank_maps(&p.query, &entries) {
                    rows.push(palette::Row { text: entries[i].row_text(), right: String::new(), enabled: true, why: entries[i].path.as_ref().map(|p| p.display().to_string()) });
                    picks.push(Pick::Map(entries[i].clone()));
                }
            }
            PalMode::GoTo => preview = Some(self.resolve_goto(&p.query).map(|(_, t)| t)),
        }
        match palette::show(ctx, &mut p, &rows, preview.as_ref()) {
            palette::Outcome::None => self.palette = Some(p),
            palette::Outcome::Close => {}
            palette::Outcome::Pick(i) => match picks.get(i) {
                Some(Pick::Cmd(c)) => self.run(*c, ctx),
                Some(Pick::Map(e)) => self.open_entry(e),
                None => self.palette = Some(p),
            },
            palette::Outcome::Submit => match self.resolve_goto(&p.query) {
                Ok((r, _)) => self.do_goto(r),
                Err(_) => self.palette = Some(p),
            },
        }
    }

    fn prefs_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.prefs.take() else { return };
        let (open, reqs) = prefs::show(ctx, &mut p, &mut self.settings, self.data_dir.is_some());
        if open {
            self.prefs = Some(p);
        }
        for r in reqs {
            match r {
                prefs::Req::OpenServer(d) => self.open_folder(&d, ctx),
                prefs::Req::OpenClient(d) => self.open_client(&d, ctx),
                prefs::Req::SetReference(d) => self.settings.reference_folder = d.map(|p| p.display().to_string()),
                prefs::Req::ClearClient => self.clear_client(),
                prefs::Req::ServerDataChanged => self.safety.invalidate(),
                prefs::Req::ClientOut(d) => self.client_out_changed(d),
                prefs::Req::ResetLayout => self.reset_layout(),
                prefs::Req::OpenDataFolder => self.run(Cmd::OpenDataFolder, ctx),
                prefs::Req::RunSetup => self.open_setup(),
            }
        }
    }

    /// The first-run / Help > Setup wizard; closing it any way counts as having seen it.
    fn wizard_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.wizard.take() else { return };
        match w.show(ctx) {
            None => self.wizard = Some(w),
            Some(e) => self.on_wizard_event(e, ctx),
        }
    }

    fn on_wizard_event(&mut self, e: WizardEvent, ctx: &egui::Context) {
        self.settings.first_run_done = true;
        if let WizardEvent::Finished(o) = e {
            self.apply_setup(o, ctx);
        }
    }

    fn apply_setup(&mut self, o: setup::Outcome, ctx: &egui::Context) {
        log::info(format!("setup finished: server {:?}, client {:?}, reference {:?}", o.server, o.client, o.reference));
        // the client first, so the folder's map list already has names
        if let Some(c) = &o.client {
            self.open_client(c, ctx);
        }
        if let Some(f) = &o.server {
            self.open_folder(f, ctx);
        }
        if let Some(r) = &o.reference {
            self.settings.reference_folder = Some(r.display().to_string());
        }
        if o.tour {
            self.help_open = true;
            self.help.goto("workflows");
        }
    }

    fn toast_overlay(&mut self, ctx: &egui::Context) {
        if let Some((t, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(4) {
                self.toast = None;
                return;
            }
            egui::Area::new(egui::Id::unique("toast")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -60.0]).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(t);
                });
            });
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn save_settings_if_changed(&mut self, ctx: &egui::Context) {
        if !self.persist {
            return;
        }
        let r = ctx.content_rect();
        self.last_known_window = [r.width(), r.height()];
        self.settings.window = self.last_known_window;
        self.settings.session = self.session_snapshot();
        if self.settings != self.last_saved && self.last_settings_write.elapsed() > Duration::from_secs(1) {
            self.last_settings_write = Instant::now();
            self.flush_settings();
        }
    }

    fn apply_look(&mut self, ctx: &egui::Context) {
        let want = (self.settings.ui_scale, self.theme_override.unwrap_or(self.settings.theme));
        if self.applied_look != Some(want) {
            ctx.set_zoom_factor(want.0);
            ctx.set_theme(match want.1 {
                crate::settings::Theme::Dark => egui::ThemePreference::Dark,
                crate::settings::Theme::Light => egui::ThemePreference::Light,
                crate::settings::Theme::System => egui::ThemePreference::System,
            });
            self.applied_look = Some(want);
        }
    }

    /// Reset layout (Preferences): panel sizes, layers and locks back to their defaults.
    fn reset_layout(&mut self) {
        let d = Settings::default();
        self.settings.left_width = d.left_width;
        self.settings.right_width = d.right_width;
        self.settings.bottom_height = d.bottom_height;
        self.settings.layers = d.layers;
        self.settings.locks = d.locks;
        self.settings.minimap_alpha = d.minimap_alpha;
        self.settings.prop_distance = d.prop_distance;
        self.r1.vo = crate::layout::ViewportOnly::default();
        self.settings.layout = d.layout;
        self.settings.layout_name = d.layout_name;
        self.layout_gen += 1;
        self.toast("layout reset");
    }

    // ------------------------------------------------------------------------------------------------------------ headless script

    fn drive_shot(&mut self, ctx: &egui::Context) {
        let Some(mut shot) = self.shot.take() else { return };
        ctx.request_repaint();
        // a run without a map (the empty state, the setup) has nothing to wait for
        let loading = self.folder.as_ref().is_some_and(|f| f.rx.is_some())
            || self.wizard.as_ref().is_some_and(|w| w.busy())
            || (!self.docs.is_empty() && self.docs.get(self.active).and_then(|d| d.map_id).and_then(|id| self.slots.get(&id)).is_none_or(|s| matches!(s, Slot::Loading(_))))
            || self.r5.loading()
            || self.safety.folder_check.is_running()
            || (self.mode3d && !self.docs.is_empty() && !self.scene3d_ready())
            || (self.client.is_some() && self.folder.is_some() && self.kinds.len() < self.folder.as_ref().map_or(0, |f| f.maps.len()) && self.kinds_rx.is_some() && self.frame_no < 600);
        let ui_ms = self.raw_ui;
        let dt = self.frame_ms_raw();
        let scenarios = self.perf_scenarios();
        let stage = std::mem::replace(&mut shot.stage, ShotStage::Wait { settled: 0 });
        shot.stage = match stage {
            ShotStage::Wait { settled } => {
                if loading {
                    ShotStage::Wait { settled: 0 }
                } else if settled < 12 {
                    ShotStage::Wait { settled: settled + 1 }
                } else {
                    if let (Some((s, i)), Some(d)) = (shot.opts.select, self.docs.get_mut(self.active)) {
                        if i < edit::count(&d.map, s) {
                            d.select_only(s, i);
                        }
                    }
                    if let Some(t) = shot.opts.tool {
                        self.tool = t;
                    }
                    for (p, r, v) in &shot.opts.paint {
                        if let Some(d) = self.docs.get_mut(self.active) {
                            let mut g = d.map.blocked.clone();
                            g.fill_circle(p[0], p[1], *r, *v);
                            d.commit_blocked("paint", &g);
                        }
                    }
                    if let Some(a) = shot.opts.generate {
                        self.open_generate(Some(a));
                        self.start_generation();
                    }
                    if shot.opts.sync {
                        self.open_sync();
                    }
                    if shot.opts.perf {
                        self.apply_scenario(&scenarios[0], 0);
                        ShotStage::Perf { scenario: 0, frames: Vec::new(), ui: Vec::new(), warm: 0 }
                    } else {
                        if let Some((x, z, zoom)) = shot.opts.view {
                            if let Some(d) = self.docs.get_mut(self.active) {
                                d.view.cx = x;
                                d.view.cz = z;
                                d.view.zoom = zoom;
                                d.view.fitted = true;
                            }
                        }
                        if self.mode3d {
                            self.apply_shot_cam(&shot.opts);
                        }
                        ShotStage::Capture { waited: 0, requested: false }
                    }
                }
            }
            ShotStage::Perf { scenario, mut frames, mut ui, warm } => {
                const WARM: u32 = 15;
                const N: usize = 240;
                if warm < WARM {
                    ShotStage::Perf { scenario, frames, ui, warm: warm + 1 }
                } else {
                    frames.push(dt);
                    ui.push(ui_ms);
                    self.apply_scenario(&scenarios[scenario], frames.len());
                    if frames.len() >= N {
                        let mut sorted = frames.clone();
                        sorted.sort_by(|a, b| a.total_cmp(b));
                        let avg = frames.iter().sum::<f32>() / frames.len() as f32;
                        let p95 = sorted[(sorted.len() as f32 * 0.95) as usize - 1];
                        let max = *sorted.last().unwrap_or(&0.0);
                        let ui_avg = ui.iter().sum::<f32>() / ui.len() as f32;
                        shot.rows.push(PerfRow { name: scenarios[scenario].0.clone(), frames: frames.len(), avg, p95, max, ui_avg });
                        if scenario + 1 < scenarios.len() {
                            self.apply_scenario(&scenarios[scenario + 1], 0);
                            ShotStage::Perf { scenario: scenario + 1, frames: Vec::new(), ui: Vec::new(), warm: 0 }
                        } else {
                            println!("frame times (vsync off), {} frames per scenario", N);
                            println!("{:<34} {:>8} {:>8} {:>8} {:>8} {:>8}", "scenario", "avg ms", "fps", "p95 ms", "max ms", "ui ms");
                            for r in &shot.rows {
                                println!("{:<34} {:>8.2} {:>8.1} {:>8.2} {:>8.2} {:>8.2}", r.name, r.avg, 1000.0 / r.avg, r.p95, r.max, r.ui_avg);
                            }
                            let _ = shot.rows.iter().map(|r| r.frames).sum::<usize>();
                            if let Some((x, z, zoom)) = shot.opts.view {
                                if let Some(d) = self.docs.get_mut(self.active) {
                                    d.view.cx = x;
                                    d.view.cz = z;
                                    d.view.zoom = zoom;
                                }
                            } else {
                                self.frame_map();
                            }
                            if self.mode3d {
                                self.apply_shot_cam(&shot.opts);
                            }
                            ShotStage::Capture { waited: 0, requested: false }
                        }
                    } else {
                        ShotStage::Perf { scenario, frames, ui, warm }
                    }
                }
            }
            ShotStage::Capture { waited, requested } => {
                if !requested {
                    if waited < 14 || self.gen_busy() || self.script3d.is_some() {
                        ShotStage::Capture { waited: waited + 1, requested: false }
                    } else {
                        let slot = shot.image.clone();
                        let done = shot.done.clone();
                        ctx.request_screenshot(move |img| {
                            *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(img);
                            done.store(true, Ordering::SeqCst);
                        });
                        ShotStage::Capture { waited, requested: true }
                    }
                } else if shot.done.load(Ordering::SeqCst) {
                    let img = shot.image.lock().unwrap_or_else(|p| p.into_inner()).take();
                    let mut code = 1;
                    if let Some(img) = img {
                        let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
                        for px in &img.pixels {
                            rgba.extend_from_slice(&px.to_array());
                        }
                        let png = crate::png_encode(img.size[0] as u32, img.size[1] as u32, &rgba);
                        match std::fs::write(&shot.opts.out, &png) {
                            Ok(()) => {
                                println!("screenshot {}x{}, {} bytes: {}", img.size[0], img.size[1], png.len(), shot.opts.out.display());
                                code = 0;
                            }
                            Err(e) => eprintln!("cannot write {}: {e}", shot.opts.out.display()),
                        }
                    }
                    std::process::exit(code);
                } else {
                    ShotStage::Capture { waited, requested: true }
                }
            }
        };
        self.shot = Some(shot);
    }

    fn frame_ms_raw(&self) -> f32 {
        self.raw_dt
    }

    fn perf_scenarios(&self) -> Vec<(String, f32)> {
        if self.r5.showing == Showing::World {
            // (name, zoom); zoom 0 = Frame all
            return vec![("World all cards".to_string(), 0.0), ("World zoom 0.5".to_string(), 0.5), ("World zoom 1".to_string(), 1.0)];
        }
        if self.mode3d {
            return vec![("3D whole map".to_string(), 0.0), ("3D 600 units away".to_string(), 600.0), ("3D 200 units away".to_string(), 200.0), ("3D 60 units away".to_string(), 60.0)];
        }
        // (name, zoom px/unit); zoom 0 = fit the whole map
        vec![("whole map (fit)".to_string(), 0.0), ("zoom 0.5 px/unit".to_string(), 0.5), ("zoom 2 px/unit".to_string(), 2.0), ("zoom 8 px/unit".to_string(), 8.0), ("zoom 32 px/unit".to_string(), 32.0)]
    }

    /// Sets the camera for a perf scenario; `step` > 0 pans a little each frame so the culling work changes.
    fn apply_scenario(&mut self, sc: &(String, f32), step: usize) {
        if self.r5.showing == Showing::World {
            let w = &mut self.r5.world;
            if sc.1 == 0.0 {
                w.zoom = 0.0;
                return;
            }
            if step == 0 {
                w.zoom = sc.1;
            }
            // sweep across the cards so the culling work changes
            let t = step as f32 * 0.05;
            w.pan = [600.0 + t.sin() * 600.0, 400.0 + (t * 0.7).cos() * 400.0];
            return;
        }
        if self.mode3d {
            self.apply_scenario3d(sc, step);
            return;
        }
        let size = self.vp.size;
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let s = d.map.size as f32;
        if sc.1 == 0.0 {
            view2d::fit_view(d, size, s);
            return;
        }
        d.view.zoom = sc.1;
        d.view.fitted = true;
        let t = step as f32 * 0.05;
        // sweep around the middle of the map
        d.view.cx = s * 0.5 + (t.sin()) * (400.0 / sc.1).min(s * 0.3);
        d.view.cz = s * 0.5 + (t * 0.7).cos() * (300.0 / sc.1).min(s * 0.3);
    }
}

/// Where a dropped path should go.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DropKind {
    Map,
    MapFolder,
    /// The folder to open as the client.
    Client(PathBuf),
    Unknown,
}

/// A `.svmap` file opens; a folder with `<number>.svmap` files is the map folder; a folder with `data.sah` or a `world` folder, or
/// `data.sah` itself, is the client; anything else is not understood.
pub(crate) fn classify_drop(p: &Path) -> DropKind {
    let lower = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if p.is_file() {
        if lower.ends_with(".svmap") {
            return DropKind::Map;
        }
        if lower == "data.sah" {
            return p.parent().map_or(DropKind::Unknown, |d| DropKind::Client(d.to_path_buf()));
        }
        return DropKind::Unknown;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return DropKind::Unknown };
    let (mut maps, mut client) = (false, false);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if setup::map_id_of(&name).is_some() && e.path().is_file() {
            maps = true;
        }
        if name == "data.sah" || (name == "world" && e.path().is_dir()) {
            client = true;
        }
    }
    if maps {
        DropKind::MapFolder
    } else if client {
        DropKind::Client(p.to_path_buf())
    } else {
        DropKind::Unknown
    }
}

pub(crate) fn position_text(p: [f32; 2]) -> String {
    format!("{:.1} {:.1}", p[0], p[1])
}

/// The GM command that moves a character: `/bmove x z map`.
pub(crate) fn gm_move_text(p: [f32; 2], map: u32) -> String {
    format!("/bmove {:.1} {:.1} {map}", p[0], p[1])
}

fn tool_name(t: Tool) -> &'static str {
    match t {
        Tool::Select => "select",
        Tool::Box => "box",
        Tool::Npc => "npc",
        Tool::Portal => "portal",
        Tool::Paint => "paint",
        Tool::Measure => "measure",
        Tool::Scatter => "scatter",
        Tool::Place => "place",
        Tool::Terrain => "terrain",
    }
}

fn tool_from_name(s: &str) -> Tool {
    Tool::ALL.into_iter().find(|t| tool_name(*t) == s).unwrap_or(Tool::Select)
}

/// The grey line under an empty outliner section: how to add the first record.
fn empty_section_hint(s: SectionId) -> &'static str {
    match s {
        SectionId::MobAreas => "draw one with Box (2)",
        SectionId::Npcs => "place one with NPC (3)",
        SectionId::Portals => "place one with Portal (4)",
        SectionId::Binds => "draw one with Box (2) set to bind area",
        SectionId::Regions => "draw one with Box (2) set to region",
        SectionId::SectionB => "click with Box (2) set to section B point",
        _ => "add one with +",
    }
}

fn tools_sync_sections(s: SectionId) -> bool {
    crate::sync::SECTIONS.contains(&s)
}

fn make_ready(ctx: &egui::Context, a: MapAssets) -> Ready {
    let mip = |mag: egui::TextureFilter| TextureOptions { magnification: mag, minification: egui::TextureFilter::Linear, wrap_mode: egui::TextureWrapMode::ClampToEdge, mipmap_mode: Some(egui::TextureFilter::Linear) };
    let load = |name: &str, p: &Option<Picture>, o: TextureOptions| p.as_ref().map(|p| ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba), o));
    let terrain = load("terrain", &a.terrain, mip(egui::TextureFilter::Linear));
    let minimap = load("minimap", &a.minimap, mip(egui::TextureFilter::Linear));
    let dungeon = load("dungeon", &a.dungeon, mip(egui::TextureFilter::Nearest));
    Ready { assets: Arc::new(a), terrain, minimap, dungeon }
}

fn layer_flags(s: &mut Settings, sec: SectionId) -> (&mut bool, &mut bool) {
    let (l, k) = (&mut s.layers, &mut s.locks);
    match sec {
        SectionId::MobAreas => (&mut l.mob_areas, &mut k.mob_areas),
        SectionId::Npcs => (&mut l.npcs, &mut k.npcs),
        SectionId::Portals => (&mut l.portals, &mut k.portals),
        SectionId::Binds => (&mut l.binds, &mut k.binds),
        SectionId::Regions => (&mut l.regions, &mut k.regions),
        SectionId::SectionB => (&mut l.points_b, &mut k.points_b),
        _ => (&mut l.section_a, &mut k.section_a),
    }
}

pub fn set_layer(s: &mut Settings, name: &str, on: bool) {
    let l = &mut s.layers;
    match name {
        "terrain" => l.terrain = on,
        "minimap" => l.minimap = on,
        "dungeon" => l.dungeon = on,
        "objects" => l.objects = on,
        "buildings3d" | "buildings" => l.buildings3d = on,
        "trees3d" | "trees" => l.trees3d = on,
        "blocked" => l.blocked = on,
        "grid" => l.grid = on,
        "ghosts" => l.ghosts = on,
        "names" => l.names = on,
        "areas" | "mob_areas" => l.mob_areas = on,
        "npcs" => l.npcs = on,
        "portals" => l.portals = on,
        "binds" => l.binds = on,
        "regions" => l.regions = on,
        "b" | "points_b" => l.points_b = on,
        "a" | "section_a" => l.section_a = on,
        "compare" => l.compare = on,
        _ => {}
    }
}

impl eframe::App for App {
    /// Tab switches 2D and 3D. egui would also use it to move the keyboard focus (it focuses the first button when nothing has
    /// focus), so the press is taken out of the input here and remembered for `handle_keys`. Tab in a text field is left alone.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.script3d_events(raw_input);
        if ctx.egui_wants_keyboard_input() || self.modal_open() {
            return;
        }
        let mut pressed = false;
        raw_input.events.retain(|e| match e {
            egui::Event::Key { key: Key::Tab, pressed: down, repeat, modifiers, .. } if !modifiers.any() => {
                pressed |= *down && !*repeat;
                false
            }
            _ => true,
        });
        self.tab_pressed |= pressed;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

impl App {
    /// One frame of the whole window. The eframe `ui` calls it; the tests call it through `egui::Context::run_ui`.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let t0 = Instant::now();
        let ctx = ui.ctx().clone();
        let dt = t0.duration_since(self.last_frame).as_secs_f32() * 1000.0;
        self.last_frame = t0;
        self.raw_dt = dt;
        self.frame_ms = self.frame_ms * 0.9 + dt * 0.1;
        self.frame_no += 1;

        // window close with unsaved maps
        if ctx.input(|i| i.viewport().close_requested()) && self.shot.is_none() {
            let mut dirty: Vec<String> = self.docs.iter().filter(|d| d.is_dirty()).map(|d| d.title()).collect();
            dirty.extend(self.c5.dirty_names());
            if !dirty.is_empty() && !self.close_confirmed {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.dialog = Some(Dialog::UnsavedExit { names: dirty });
            } else {
                self.flush_settings();
                self.recovery_remove_all();
                log::info("exit");
            }
        }
        self.apply_look(&ctx);
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.handle_dropped(dropped, &ctx);
        }

        self.hover_rec = None;
        self.poll(&ctx);
        self.ensure_client_parts();
        self.handle_keys(&ctx);
        self.ensure_assets(&ctx);
        self.sync_terrain_views(&ctx);
        self.run_validation();
        self.disk_check(false);
        self.autosave_tick(false);
        self.refused_toasts();
        self.compare_update();
        self.folder_check_tick(&ctx);
        self.step_save_queue();

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        // viewport only (F11): the toolbar, the tool palette and the status bar go with the panels
        let chrome = !self.r1.vo.active();
        self.r1.drawn_toolbar = None;
        self.r1.drawn_status = false;
        self.r1.drawn_palette = None;
        if chrome {
            let r = egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
            self.r1.drawn_toolbar = Some(r.response.rect);
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        }
        self.draw_panels(ui);
        if chrome {
            self.tool_palette(ui);
        }
        egui::CentralPanel::default().show(ui, |ui| self.central(ui));

        self.dialogs(&ctx);
        self.picker_window(&ctx);
        self.generate_window(&ctx);
        self.new_map_window(&ctx);
        self.ini_window(&ctx);
        self.add_window(&ctx);
        self.share_window(&ctx);
        self.sync_window(&ctx);
        self.help_windows(&ctx);
        self.palette_ui(&ctx);
        self.prefs_window(&ctx);
        self.layout_windows(&ctx);
        self.wizard_window(&ctx);
        self.recovery_window(&ctx);
        self.backups_window(&ctx);
        self.bulk_window(&ctx);
        self.monster_pick_window(&ctx);
        self.links_window(&ctx);
        self.prompt_window(&ctx);
        self.process_drops();
        self.png_tick(&ctx);
        self.place_browser(&ctx);
        self.toast_overlay(&ctx);
        self.save_settings_if_changed(&ctx);
        if self.crosshair.is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(3)) {
            self.crosshair = None;
        }
        if self.crosshair.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        // the 2 s changed-on-disk check, autosave and the recovery heartbeat also run while nobody touches the window
        if !self.docs.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        self.raw_ui = t0.elapsed().as_secs_f32() * 1000.0;
        self.ui_ms = self.ui_ms * 0.9 + self.raw_ui * 0.1;
        self.drive_shot(&ctx);
    }
}
