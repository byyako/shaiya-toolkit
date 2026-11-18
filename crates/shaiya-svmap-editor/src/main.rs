//! Shaiya SVMap Editor: the desktop program. An eframe window with an egui UI and a wgpu 3D view; the engine (documents, pairing,
//! checks, install) is in `shaiya-svmap-tools`. There are no subcommands: the command-line tools are `shaiya svmap ...`.
//! `--smoke [out.png]` renders a few frames of a coloured triangle through the wgpu paint callback, checks its pixels and exits
//! (0 = triangle found); it reads and writes no settings and reports through the exit code and the PNG only.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod backups;
mod commands;
mod gpu3d;
mod help;
mod help_text;
mod icons;
mod inspector;
mod layout;
mod log;
mod palette;
mod place_ui;
mod prefs;
mod recovery;
mod scene3d;
mod settings;
mod setup;
mod terrain_ui;
mod theme;
mod triangle;
mod view2d;
mod view3d;

// The engine modules live in `shaiya-svmap-tools`; they are imported here so the editor code keeps writing `crate::doc::Doc`.
#[allow(unused_imports)]
use shaiya_svmap_tools::{
    assets, bulk, client_files, client_rules, compare, cross_rules, doc, edit, export, fixes, folderdiff, generate, heat, ini_doc, mapini, math3d, measure, names, newmap, pair, place,
    problems, scatter, search, server_data, share, spawns, sync, terrain, world,
};
/// Temp folders and tiny test files; only in the editor's own tests (the dev-dependency turns the `testing` feature on).
#[cfg(test)]
use shaiya_svmap_tools::testutil;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

struct SmokeApp {
    frame_no: u64,
    smoke: Option<Smoke>,
    last_frame: std::time::Instant,
    frame_ms: f32,
    adapter_line: String,
}

struct Smoke {
    requested: bool,
    result: Arc<Mutex<Option<ShotResult>>>,
    done: Arc<AtomicBool>,
}

struct ShotResult {
    triangle_px: usize,
}

impl SmokeApp {
    fn new(cc: &eframe::CreationContext<'_>, smoke: Option<Smoke>) -> Self {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .expect("eframe was built with the wgpu renderer");
        triangle::install(rs);
        let info = rs.adapter.get_info();
        let adapter_line = format!(
            "{} | backend {:?} | {:?} | driver {} {}",
            info.name, info.backend, info.device_type, info.driver, info.driver_info
        );
        SmokeApp {
            frame_no: 0,
            smoke,
            last_frame: std::time::Instant::now(),
            frame_ms: 0.0,
            adapter_line,
        }
    }
}

impl eframe::App for SmokeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32() * 1000.0;
        self.last_frame = now;
        self.frame_ms = self.frame_ms * 0.9 + dt * 0.1;
        self.frame_no += 1;

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Shaiya SVMap Editor - smoke check");
                ui.label(format!("frame {} ({:.1} ms)", self.frame_no, self.frame_ms));
            });
            ui.label(&self.adapter_line);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let (rect, _resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                triangle::TriangleCallback,
            ));
        });

        if let Some(s) = &mut self.smoke {
            let ctx = ui.ctx().clone();
            if self.frame_no == 12 && !s.requested {
                s.requested = true;
                let result = s.result.clone();
                let done = s.done.clone();
                ctx.request_screenshot(move |img| {
                    *result.lock().unwrap() = Some(analyse(&img));
                    save_png_if(&img);
                    done.store(true, Ordering::SeqCst);
                });
            }
            if s.done.load(Ordering::SeqCst) {
                let r = s.result.lock().unwrap().take().unwrap();
                let ok = r.triangle_px > 1000;
                std::process::exit(if ok { 0 } else { 1 });
            }
            ctx.request_repaint();
        }
    }
}

static PNG_OUT: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn save_png_if(img: &egui::ColorImage) {
    if let Some(p) = PNG_OUT.lock().unwrap().clone() {
        let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
        for px in &img.pixels {
            rgba.extend_from_slice(&px.to_array());
        }
        if let Err(e) = std::fs::write(&p, png_encode(img.size[0] as u32, img.size[1] as u32, &rgba)) {
            eprintln!("could not write {}: {e}", p.display());
        }
    }
}

/// The triangle's vertex colours are pure red, green and blue; the background is egui's dark grey. A pixel counts as
/// "triangle" when one channel clearly dominates.
fn analyse(img: &egui::ColorImage) -> ShotResult {
    let mut n = 0;
    for px in &img.pixels {
        let [r, g, b, _] = px.to_array();
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        if mx > 120 && mx - mn > 80 {
            n += 1;
        }
    }
    ShotResult { triangle_px: n }
}

/// PNG encoder (filter 0, zlib by miniz_oxide): enough for a screenshot, no image crate.
pub(crate) fn png_encode(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB88320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

const EDITOR_USAGE: &str = "shaiya-svmap-editor [-h|--help] [-V|--version] [file.svmap ...] [--folder <Map dir>] [--client <client dir>] [--map <id> ...]
  headless: --shot <out.png> [--perf] [--layers +name,-name,...] [--view x,z,zoom] [--select section:index] [--size WxH]
            (a run with --shot never reads or writes the settings file unless --data-dir is given)
  layers: terrain minimap dungeon objects buildings trees blocked grid ghosts names areas npcs portals binds regions b a compare
            (objects = the 2D dots; buildings and trees = the 3D models)  --prop-distance <units>  (3D model cut-off, default 800)
            --help-page <n>  (start with the Help window open on page n, 0 = the first page)
            --show setup[:1-3]|help[:page]|palette|quickopen[:text]|goto[:text]|shortcuts|prefs[:folders]|problems|pair[:sync]|place|terrain|ini[:zone]|add-map[:source]|share[:folder]|compare|recovery|find[:query]|links|batch|layout:<name>|dock:<tab>|collapsed:<panel>|viewport-only|heat:density|heat:level|spawns[:folder[:findings|:never]]|scatter[:poly]|bulk[:rule]|measure[:radius]|rulers  (open that window or layout at start;
            problems, pair and compare make a few demo edits in the open map first (pair edits the client half too, pair:sync opens Sync from client), recovery writes an entry and offers it, needs --data-dir;
            find opens the Find tab with the query, links the portal link table, batch selects the first monster areas;
            layout:Simple|Editing|Review|<saved name>, dock:problems|problems-info|changes|find|spawns (problems-info also shows the info findings), collapsed:browser|inspector|dock;
            heat:density|level turns the heatmap on, spawns opens the Spawns tab, spawns:folder its all-maps scope;
            scatter picks the Scatter tool with a region in the middle of the map, bulk opens the Bulk edit window with the rule,
            measure picks the Measure tool with a path (measure:radius a circle), rulers shows the rulers,
            place picks the Placements tool with the building nearest the map centre turned and selected, and the Models window open,
            terrain picks the Terrain tool after a plateau and a hill in the middle of the map and shows the brush ring,
            ini opens the Map.ini editor on a zone, add-map opens Add a map with a client map as its source, share opens Share client changes; the first two need the server data folder next to the map folder)
            --theme dark|light  (this run only)
  sections for --select: area npc portal bind region b a
            --tool select|box|npc|portal|paint|measure|scatter|place|terrain  --paint x,z,radius[,0]  (brush dab, repeatable)
            --generate dungeon|slope  (open Generate blocked cells with its preview)  --sync  (open Sync from client)
            --new-from-client <id>  (start with a new map made from that client map)
            --3d  (start in the 3D view)  --cam yaw,pitch[,distance[,x,z]]  (3D camera, degrees; yaw 0 = north, pitch -90 = straight down;
            --select first frames the selection)
            --drag3d section:index:dx,dy  (press on that record in the 3D view, drag by dx,dy pixels with synthetic mouse events,
            release, and print what happened)
  data folder: --data-dir <dir>  (the toolkit data root: settings, log, recovery and backups live in <dir>/shaiya-svmap-editor)
            --no-settings  (no data folder at all: no settings, session, log or setup wizard)
  other: --smoke [out.png]  (draw a test triangle, check it, write the picture, exit 0 when it is found; no settings are read or written)
  A release build on Windows has no console: to read this text or the version, redirect the output (shaiya-svmap-editor --help > help.txt) or pipe it.";

fn parse_section(s: &str) -> Option<shaiya_svmap::SectionId> {
    use shaiya_svmap::SectionId as S;
    Some(match s {
        "area" | "areas" => S::MobAreas,
        "npc" | "npcs" => S::Npcs,
        "portal" | "portals" => S::Portals,
        "bind" | "binds" => S::Binds,
        "region" | "regions" => S::Regions,
        "b" => S::SectionB,
        "a" => S::SectionA,
        _ => return None,
    })
}

fn parse_editor_args(args: &[String]) -> Result<app::StartOptions, String> {
    let mut o = app::StartOptions::default();
    let mut shot_out: Option<std::path::PathBuf> = None;
    let (mut perf, mut view, mut select) = (false, None, None);
    let (mut tool, mut paint, mut generate, mut sync) = (None, Vec::new(), None, false);
    let (mut view3d, mut cam, mut drag3d) = (false, None, None);
    let mut i = 0;
    let next = |i: &mut usize, what: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| format!("{what} needs a value"))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--folder" => o.folder = Some(next(&mut i, "--folder")?.into()),
            "--client" => o.client = Some(next(&mut i, "--client")?.into()),
            "--map" => o.maps.push(next(&mut i, "--map")?.parse().map_err(|_| "--map needs a number")?),
            "--shot" => shot_out = Some(next(&mut i, "--shot")?.into()),
            "--perf" => perf = true,
            "--layers" => {
                for t in next(&mut i, "--layers")?.split(',') {
                    let (on, name) = match t.strip_prefix('-') {
                        Some(n) => (false, n),
                        None => (true, t.strip_prefix('+').unwrap_or(t)),
                    };
                    o.layer_edits.push((name.to_string(), on));
                }
            }
            "--data-dir" => o.data_dir = Some(next(&mut i, "--data-dir")?.into()),
            "--show" => o.show = Some(next(&mut i, "--show")?),
            "--theme" => {
                o.theme = Some(match next(&mut i, "--theme")?.as_str() {
                    "dark" => crate::settings::Theme::Dark,
                    "light" => crate::settings::Theme::Light,
                    _ => return Err("--theme needs dark or light".to_string()),
                })
            }
            "--help-page" => o.help_page = Some(next(&mut i, "--help-page")?.parse().map_err(|_| "--help-page needs a number (0 = Shortcuts)")?),
            "--prop-distance" => o.prop_distance = Some(next(&mut i, "--prop-distance")?.parse().map_err(|_| "--prop-distance needs a number")?),
            "--view" => {
                let v = next(&mut i, "--view")?;
                let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if p.len() != 3 {
                    return Err("--view needs x,z,zoom".to_string());
                }
                view = Some((p[0], p[1], p[2]));
            }
            "--select" => {
                let v = next(&mut i, "--select")?;
                let (s, n) = v.split_once(':').ok_or("--select needs section:index")?;
                select = Some((parse_section(s).ok_or("unknown section for --select")?, n.parse::<usize>().map_err(|_| "bad index")?));
            }
            "--size" => {
                let v = next(&mut i, "--size")?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                o.window = Some([w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?]);
            }
            "--tool" => {
                tool = Some(match next(&mut i, "--tool")?.as_str() {
                    "select" => view2d::Tool::Select,
                    "box" => view2d::Tool::Box,
                    "npc" => view2d::Tool::Npc,
                    "portal" => view2d::Tool::Portal,
                    "paint" => view2d::Tool::Paint,
                    "measure" => view2d::Tool::Measure,
                    "scatter" => view2d::Tool::Scatter,
                    "place" => view2d::Tool::Place,
                    "terrain" => view2d::Tool::Terrain,
                    _ => return Err("--tool needs select, box, npc, portal, paint, measure, scatter, place or terrain".to_string()),
                })
            }
            "--paint" => {
                let v = next(&mut i, "--paint")?;
                let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if p.len() < 3 {
                    return Err("--paint needs x,z,radius[,0 to free]".to_string());
                }
                paint.push(([p[0], p[1]], p[2], p.get(3).copied().unwrap_or(1.0) != 0.0));
            }
            "--generate" => {
                generate = Some(match next(&mut i, "--generate")?.as_str() {
                    "dungeon" => app::tools::GenAlgo::Dungeon,
                    "slope" => app::tools::GenAlgo::Slope,
                    _ => return Err("--generate needs dungeon or slope".to_string()),
                })
            }
            "--sync" => sync = true,
            "--3d" => view3d = true,
            "--drag3d" => {
                let v = next(&mut i, "--drag3d")?;
                let mut it = v.splitn(3, ':');
                let (Some(s), Some(n), Some(d)) = (it.next(), it.next(), it.next()) else { return Err("--drag3d needs section:index:dx,dy".to_string()) };
                let (dx, dz) = d.split_once(',').ok_or("--drag3d needs section:index:dx,dy")?;
                drag3d = Some((parse_section(s).ok_or("unknown section for --drag3d")?, n.parse::<usize>().map_err(|_| "bad index")?, [dx.parse::<f32>().map_err(|_| "bad dx")?, dz.parse::<f32>().map_err(|_| "bad dy")?]));
            }
            "--cam" => {
                let v = next(&mut i, "--cam")?;
                let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if p.len() < 2 || p.len() == 4 || p.len() > 5 {
                    return Err("--cam needs yaw,pitch[,distance[,x,z]] (degrees; yaw 0 looks north, pitch -90 straight down)".to_string());
                }
                cam = Some(p);
            }
            "--new-from-client" => o.new_from_client = Some(next(&mut i, "--new-from-client")?.parse().map_err(|_| "--new-from-client needs a map id")?),
            "--no-settings" => o.no_settings = true,
            a if a.starts_with("--") => return Err(format!("unknown option {a}")),
            f => o.files.push(f.into()),
        }
        i += 1;
    }
    if let Some(out) = shot_out {
        // a screenshot run leaves the user's settings alone unless it was given a data folder of its own
        if o.data_dir.is_none() {
            o.no_settings = true;
        }
        o.shot = Some(app::ShotOpts { out, perf, view, select, tool, paint, generate, sync, view3d, cam, drag3d });
    }
    Ok(o)
}

/// The first words of the old command-line tools: they moved to `shaiya svmap`.
const MOVED_WORDS: [&str; 9] = ["check", "dump", "build", "roundtrip", "diff", "spawns", "bulk", "wld-check", "help"];

const MOVED_MESSAGE: &str = "the command-line tools moved: run \"shaiya svmap <word> ...\" (see shaiya svmap --help)";

/// `Some(word)` when the first argument is one of the moved command words.
fn moved_word(args: &[String]) -> Option<&str> {
    args.first().map(String::as_str).filter(|w| MOVED_WORDS.contains(w))
}

/// Takes `--smoke [out.png]` out of the arguments, wherever it stands. The result is `None` without `--smoke`, else the optional PNG path.
fn take_smoke(args: &mut Vec<String>) -> Option<Option<std::path::PathBuf>> {
    let at = args.iter().position(|a| a == "--smoke")?;
    args.remove(at);
    let out = (at < args.len() && !args[at].starts_with("--")).then(|| std::path::PathBuf::from(args.remove(at)));
    Some(out)
}

/// The editor's data folder: `<root>/shaiya-svmap-editor`, the root being `--data-dir` or the toolkit's data root (`SHAIYA_TOOLKIT_HOME`,
/// the portable folder, `%APPDATA%\shaiya-toolkit`, ...). None when no root can be determined.
fn data_folder(data_dir_arg: Option<std::path::PathBuf>, env: &shaiya_ui::settings::Env, os: shaiya_ui::settings::Os) -> Option<std::path::PathBuf> {
    let root = data_dir_arg.or_else(|| env.root(os))?;
    Some(shaiya_ui::settings::Paths::new(root, TOOL_ID).tool_dir)
}

/// The settings folder name and the data-folder name of this program.
const TOOL_ID: &str = "shaiya-svmap-editor";

fn version_line() -> String {
    format!("shaiya-svmap-editor {} (git {}, {})", env!("CARGO_PKG_VERSION"), env!("SHAIYA_GIT_HASH"), env!("SHAIYA_TARGET"))
}

/// A release build on Windows has no console, so the message is also shown in a dialog there.
fn tell_moved() {
    eprintln!("error: {MOVED_MESSAGE}");
    #[cfg(all(windows, not(debug_assertions)))]
    {
        rfd::MessageDialog::new().set_level(rfd::MessageLevel::Info).set_title("Shaiya SVMap Editor").set_description(MOVED_MESSAGE).set_buttons(rfd::MessageButtons::Ok).show();
    }
}

fn window_icon() -> egui::IconData {
    egui::IconData { rgba: include_bytes!("../../../assets/icons/shaiya-svmap-editor-64.rgba").to_vec(), width: 64, height: 64 }
}

fn main() -> eframe::Result {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if moved_word(&args).is_some() {
        tell_moved();
        std::process::exit(2);
    }
    if let Some(first) = args.first() {
        if first == "-h" || first == "--help" {
            println!("{EDITOR_USAGE}\n\nCommand-line tools: shaiya svmap --help");
            return Ok(());
        }
        if first == "-V" || first == "--version" {
            println!("{}", version_line());
            return Ok(());
        }
    }
    if let Some(out) = take_smoke(&mut args) {
        *PNG_OUT.lock().unwrap() = out;
        let smoke = Some(Smoke { requested: false, result: Arc::new(Mutex::new(None)), done: Arc::new(AtomicBool::new(false)) });
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 600.0]).with_title("Shaiya SVMap Editor").with_icon(window_icon()),
            ..Default::default()
        };
        return eframe::run_native("Shaiya SVMap Editor", options, Box::new(|cc| Ok(Box::new(SmokeApp::new(cc, smoke)))));
    }
    let mut opts = match parse_editor_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\n{EDITOR_USAGE}\n\nCommand-line tools: shaiya svmap --help");
            std::process::exit(2);
        }
    };
    let data_dir = if opts.no_settings { None } else { data_folder(opts.data_dir.take(), &shaiya_ui::settings::Env::from_process(), shaiya_ui::settings::Os::CURRENT) };
    opts.no_data_folder = !opts.no_settings && data_dir.is_none();
    opts.data_dir = data_dir.clone();
    let settings = data_dir.as_deref().map(settings::file_in).map_or_else(settings::Settings::default, |p| settings::Settings::load_from(&p));
    if let Some(d) = &data_dir {
        log::init(d);
        log::install_panic_hook();
    }
    let size = opts.window.unwrap_or(settings.window);
    let perf = opts.shot.as_ref().is_some_and(|s| s.perf);
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size(size).with_title("Shaiya SVMap Editor").with_icon(window_icon()),
        ..Default::default()
    };
    if perf {
        // frame times without the vsync wait
        options.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native("Shaiya SVMap Editor", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, opts)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_signature_and_chunks() {
        let png = png_encode(2, 2, &[255u8; 16]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn analyse_finds_saturated_pixels() {
        let mut img = egui::ColorImage::filled([10, 10], egui::Color32::from_gray(40));
        img.pixels[0] = egui::Color32::from_rgb(255, 0, 0);
        assert_eq!(analyse(&img).triangle_px, 1);
    }
}

#[cfg(test)]
mod arg_tests {
    use super::*;
    use shaiya_ui::settings::{Env, Os};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_old_command_words_are_caught_and_files_are_not() {
        for w in MOVED_WORDS {
            assert_eq!(moved_word(&strings(&[w, "x"])), Some(w));
        }
        assert_eq!(moved_word(&strings(&["35.svmap"])), None);
        assert_eq!(moved_word(&strings(&["--folder", "check"])), None);
        assert_eq!(moved_word(&[]), None);
    }

    #[test]
    fn smoke_is_taken_from_anywhere() {
        let mut a = strings(&["--smoke", "out.png", "--no-settings"]);
        assert_eq!(take_smoke(&mut a), Some(Some("out.png".into())));
        assert_eq!(a, strings(&["--no-settings"]));
        let mut a = strings(&["--theme", "dark", "--smoke"]);
        assert_eq!(take_smoke(&mut a), Some(None));
        assert_eq!(a, strings(&["--theme", "dark"]));
        let mut a = strings(&["--smoke", "--theme", "dark"]);
        assert_eq!(take_smoke(&mut a), Some(None));
        assert_eq!(a, strings(&["--theme", "dark"]));
        let mut a = strings(&["x.svmap"]);
        assert_eq!(take_smoke(&mut a), None);
    }

    #[test]
    fn the_data_folder_is_the_tool_folder_of_the_root_and_none_without_a_root() {
        let none = Env::default();
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            assert_eq!(data_folder(None, &none, os), None, "no home, no portable file, no APPDATA: nothing is stored");
        }
        assert_eq!(data_folder(Some("D:/dd".into()), &none, Os::Windows), Some(std::path::PathBuf::from("D:/dd").join("shaiya-svmap-editor")));
        let env = Env { toolkit_home: Some("D:/home".into()), ..Env::default() };
        assert_eq!(data_folder(None, &env, Os::Linux), Some(std::path::PathBuf::from("D:/home").join("shaiya-svmap-editor")));
        // the argument wins over the environment
        assert_eq!(data_folder(Some("D:/dd".into()), &env, Os::Linux), Some(std::path::PathBuf::from("D:/dd").join("shaiya-svmap-editor")));
    }

    #[test]
    fn the_version_line_names_the_program_and_the_build() {
        let v = version_line();
        assert!(v.starts_with("shaiya-svmap-editor ") && v.contains("(git ") && v.ends_with(')'), "{v}");
    }
}
