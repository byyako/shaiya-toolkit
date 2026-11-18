//! The Preferences window (`Ctrl+,`): what to restore, hints, autosave and backup numbers, UI scale and theme, the three folders
//! with the same report the setup shows, the snap steps, Reset layout and Open data folder. It edits the `Settings` directly;
//! things that need the app (opening a folder, the layout reset) come back as requests.

use std::path::{Path, PathBuf};

use egui::RichText;

use crate::settings::{Settings, Theme, SNAP_STEPS_DEFAULT};
use crate::setup::{inspect_client, inspect_server, ClientReport, Inspect, ServerReport, Tone};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Appearance,
    Folders,
    Snap,
}

// Tooltips of the Preferences window (the string lint reads `TIPS`).
const PREF_RESTORE: &str = "The tabs of the last run come back when you start without a file or folder.";
const PREF_HINTS: &str = "The tool hint line and the how-to texts in empty lists.";
const PREF_FRAME_TIME: &str = "Show the frame time at the right of the status bar.";
const PREF_ORBIT: &str = "A left drag in the 3D view that starts on the ground turns the camera around that point.";
const PREF_SMOOTH: &str = "Frame, Go to and the compass move the 3D camera over 0.2 seconds.";
const PREF_AUTOSAVE: &str = "Unsaved work goes to the recovery folder. It is offered back after a crash.";
const PREF_BACKUPS: &str = "Timestamped copies of every save. File > Restore from backup... brings one back.";
const PREF_RESET: &str = "Panels, layers and locks go back to their defaults. The layout becomes Simple.";
const PREF_OPEN_DATA: &str = "Settings and the log file.";
const PREF_NO_DATA: &str = "This run has no data folder (--no-settings).";
const PREF_SNAP: &str = "The steps in the Snap list, in world units. Arrow keys nudge by the snap step.";
const PREF_SERVER_DATA: &str = "The folder with Map.ini and NpcQuest.SData. Empty: the map folder's parent, when it holds Map.ini.";
const PREF_AFTERGLOW: &str = "A folder with mobs.psv, or the file. Empty: monster ids are not checked against the server.";
const PREF_TARGET: &str = "A map id without a map file is an error for the original server and an info for Afterglow.";
const PREF_CLIENT_OUT: &str = "Edited client files go here as data/world/<id>.wld. The client folder is never written.";
const PREF_CODEPAGE: &str = "The code page of the names in .wld files. The EP4 client uses cp949.";

/// Every tooltip and help line of the window, for the string lint test.
#[cfg(test)]
pub const TIPS: &[&str] = &[PREF_RESTORE, PREF_HINTS, PREF_FRAME_TIME, PREF_ORBIT, PREF_SMOOTH, PREF_AUTOSAVE, PREF_BACKUPS, PREF_RESET, PREF_OPEN_DATA, PREF_NO_DATA, PREF_SNAP, PREF_SERVER_DATA, PREF_AFTERGLOW, PREF_TARGET, PREF_CLIENT_OUT, PREF_CODEPAGE];

pub enum Req {
    OpenServer(PathBuf),
    OpenClient(PathBuf),
    SetReference(Option<PathBuf>),
    /// Forget the client folder (the open tabs stay, names fall back to numbers).
    ClearClient,
    /// The server data folder, the Afterglow export or the target server changed: the folder check runs again.
    ServerDataChanged,
    /// The staging folder for edited client files was picked (already checked against the client folder) or cleared.
    ClientOut(Option<PathBuf>),
    ResetLayout,
    OpenDataFolder,
    RunSetup,
}

pub struct Prefs {
    pub tab: Tab,
    server: Inspect<ServerReport>,
    client: Inspect<ClientReport>,
    reference: Inspect<ServerReport>,
    snap_text: String,
    snap_error: Option<String>,
    /// The scale being dragged; applied when the drag ends so the slider does not move under the cursor.
    scale_edit: f32,
    /// Why the last picked staging folder was refused.
    out_error: Option<String>,
}

impl Prefs {
    pub fn new(s: &Settings) -> Prefs {
        Prefs { tab: Tab::General, server: Inspect::default(), client: Inspect::default(), reference: Inspect::default(), snap_text: snap_text(&s.snap_steps), snap_error: None, scale_edit: s.ui_scale, out_error: None }
    }
}

pub fn snap_text(steps: &[f32]) -> String {
    steps.iter().map(|s| format!("{s}")).collect::<Vec<_>>().join(", ")
}

/// A comma separated list of positive numbers.
pub fn parse_snap_steps(text: &str) -> Result<Vec<f32>, String> {
    let mut v = Vec::new();
    for part in text.split([',', ';', ' ']).filter(|p| !p.is_empty()) {
        let n: f32 = part.parse().map_err(|_| format!("\"{part}\" is not a number"))?;
        if !n.is_finite() || n <= 0.0 || n > 1024.0 {
            return Err(format!("{part} is not a step between 0 and 1024"));
        }
        v.push(n);
    }
    if v.is_empty() {
        return Err("give at least one step".to_string());
    }
    if v.len() > 16 {
        return Err("at most 16 steps".to_string());
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v.dedup();
    Ok(v)
}

fn lines(ui: &mut egui::Ui, l: &[(Tone, String)]) {
    for (t, s) in l {
        ui.label(RichText::new(s).color(t.colour(ui)).small());
    }
}

/// Draws the window. Returns false when it was closed, and the requests.
pub fn show(ctx: &egui::Context, p: &mut Prefs, s: &mut Settings, has_data_dir: bool) -> (bool, Vec<Req>) {
    let mut open = true;
    let mut reqs = Vec::new();
    let server_before = (s.server_data.clone(), s.afterglow_defs_path.clone(), s.target_server);
    p.server.poll();
    p.client.poll();
    p.reference.poll();
    egui::Window::new("Preferences").open(&mut open).default_size([560.0, 420.0]).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut p.tab, Tab::General, "General");
            ui.selectable_value(&mut p.tab, Tab::Appearance, "Appearance");
            ui.selectable_value(&mut p.tab, Tab::Folders, "Folders");
            ui.selectable_value(&mut p.tab, Tab::Snap, "Snap steps");
        });
        ui.separator();
        match p.tab {
            Tab::General => {
                ui.checkbox(&mut s.session.restore, "Restore the open maps, cameras and tool on start").on_hover_text(PREF_RESTORE);
                ui.checkbox(&mut s.hints, "Show hints").on_hover_text(PREF_HINTS);
                ui.checkbox(&mut s.show_frame_time, "Show frame time in the status bar").on_hover_text(PREF_FRAME_TIME);
                ui.checkbox(&mut s.orbit_at_cursor, "3D: orbit around the point under the cursor").on_hover_text(PREF_ORBIT);
                ui.checkbox(&mut s.smooth_camera, "3D: smooth camera moves").on_hover_text(PREF_SMOOTH);
                ui.horizontal(|ui| {
                    ui.label("Autosave every");
                    ui.add(egui::DragValue::new(&mut s.autosave_secs).range(0..=3600).suffix(" s"));
                    ui.label(RichText::new("0 = off, at least 15").weak());
                })
                .response
                .on_hover_text(PREF_AUTOSAVE);
                if s.autosave_secs != 0 && s.autosave_secs < 15 {
                    s.autosave_secs = 15;
                }
                ui.horizontal(|ui| {
                    ui.label("Backups kept per map");
                    ui.add(egui::DragValue::new(&mut s.backups_keep).range(0..=500));
                    ui.label(RichText::new("0 = off").weak());
                })
                .response
                .on_hover_text(PREF_BACKUPS);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Reset layout").on_hover_text(PREF_RESET).clicked() {
                        reqs.push(Req::ResetLayout);
                    }
                    if ui.add_enabled(has_data_dir, egui::Button::new("Open data folder")).on_hover_text(PREF_OPEN_DATA).on_disabled_hover_text(PREF_NO_DATA).clicked() {
                        reqs.push(Req::OpenDataFolder);
                    }
                    if ui.button("Run setup...").clicked() {
                        reqs.push(Req::RunSetup);
                    }
                });
            }
            Tab::Appearance => {
                ui.horizontal(|ui| {
                    ui.label("UI scale");
                    let r = ui.add(egui::Slider::new(&mut p.scale_edit, 0.75..=2.0).fixed_decimals(2));
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        s.ui_scale = p.scale_edit;
                    }
                    if ui.small_button("100 %").clicked() {
                        p.scale_edit = 1.0;
                        s.ui_scale = 1.0;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme").selected_text(crate::settings::theme_label(s.theme)).show_ui(ui, |ui| {
                        for t in Theme::ALL {
                            ui.selectable_value(&mut s.theme, t, crate::settings::theme_label(t));
                        }
                    });
                });
                ui.label(RichText::new("Map colours stay the same in both themes.").weak().small());
            }
            Tab::Folders => {
                let server_text = s.svmap_folder.clone().unwrap_or_default();
                let client_text = s.client_folder.clone().unwrap_or_default();
                let ref_text = s.reference_folder.clone().unwrap_or_default();
                if !server_text.is_empty() {
                    let path = PathBuf::from(&server_text);
                    p.server.request(&server_text, Some(ctx), move || inspect_server(&path));
                }
                let maps: Vec<u32> = p.server.done.as_ref().filter(|r| r.valid()).map(|r| r.maps.clone()).unwrap_or_default();
                if !client_text.is_empty() {
                    let path = PathBuf::from(&client_text);
                    let m2 = maps.clone();
                    p.client.request(&format!("{client_text}|{}", crate::setup::ranges_text(&maps)), Some(ctx), move || inspect_client(&path, &m2));
                }
                if !ref_text.is_empty() {
                    let path = PathBuf::from(&ref_text);
                    p.reference.request(&ref_text, Some(ctx), move || inspect_server(&path));
                }
                ui.label(RichText::new("Server map folder").strong());
                ui.label(if server_text.is_empty() { "(none)".to_string() } else { server_text.clone() });
                if ui.button("Change...").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        reqs.push(Req::OpenServer(d));
                        p.server.clear();
                    }
                }
                if p.server.busy() {
                    ui.spinner();
                }
                if let Some(r) = &p.server.done {
                    lines(ui, &r.lines());
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Client data").strong());
                ui.label(if client_text.is_empty() { "(none)".to_string() } else { client_text.clone() });
                ui.horizontal(|ui| {
                    if ui.button("Change...").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            reqs.push(Req::OpenClient(d));
                            p.client.clear();
                        }
                    }
                    if ui.add_enabled(!client_text.is_empty(), egui::Button::new("Clear")).clicked() {
                        reqs.push(Req::ClearClient);
                        p.client.clear();
                    }
                });
                if p.client.busy() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("reading the client...");
                    });
                }
                if let Some(r) = &p.client.done {
                    lines(ui, &r.lines());
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Reference folder (an untouched copy of the maps, to compare against)").strong());
                ui.label(if ref_text.is_empty() { "(none)".to_string() } else { ref_text.clone() });
                ui.horizontal(|ui| {
                    if ui.button("Change...").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            reqs.push(Req::SetReference(Some(d)));
                            p.reference.clear();
                        }
                    }
                    if ui.add_enabled(!ref_text.is_empty(), egui::Button::new("Clear")).clicked() {
                        reqs.push(Req::SetReference(None));
                        p.reference.clear();
                    }
                });
                if let Some(r) = &p.reference.done {
                    lines(ui, &r.lines());
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Server data folder (Map.ini, NpcQuest.SData, Obelisk.ini)").strong()).on_hover_text(PREF_SERVER_DATA);
                let sd = s.server_data.clone().unwrap_or_default();
                ui.label(if sd.is_empty() { "(none: the parent of the map folder, if it holds Map.ini)".to_string() } else { sd.clone() });
                ui.horizontal(|ui| {
                    if ui.button("Change...").on_hover_text(PREF_SERVER_DATA).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            s.server_data = Some(d.display().to_string());
                        }
                    }
                    if ui.add_enabled(!sd.is_empty(), egui::Button::new("Clear")).clicked() {
                        s.server_data = None;
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Afterglow defs export (mobs.psv)").strong()).on_hover_text(PREF_AFTERGLOW);
                let ag = s.afterglow_defs_path.clone().unwrap_or_default();
                ui.label(if ag.is_empty() { "(none)".to_string() } else { ag.clone() });
                ui.horizontal(|ui| {
                    if ui.button("Change...").on_hover_text(PREF_AFTERGLOW).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            s.afterglow_defs_path = Some(d.display().to_string());
                        }
                    }
                    if ui.add_enabled(!ag.is_empty(), egui::Button::new("Clear")).clicked() {
                        s.afterglow_defs_path = None;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Target server");
                    egui::ComboBox::from_id_salt("target-server").selected_text(s.target_server.label()).show_ui(ui, |ui| {
                        for t in crate::settings::TargetServer::ALL {
                            ui.selectable_value(&mut s.target_server, t, t.label());
                        }
                    })
                    .response
                    .on_hover_text(PREF_TARGET);
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Client output folder (edited client files)").strong()).on_hover_text(PREF_CLIENT_OUT);
                let out = s.client_out.clone().unwrap_or_default();
                ui.label(if out.is_empty() { "(none: asked on the first client save)".to_string() } else { out.clone() });
                ui.horizontal(|ui| {
                    if ui.button("Change...").on_hover_text(PREF_CLIENT_OUT).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            match if client_text.is_empty() { Ok(()) } else { crate::client_files::check_out_dir(&d, Path::new(&client_text)) } {
                                Ok(()) => {
                                    p.out_error = None;
                                    reqs.push(Req::ClientOut(Some(d)));
                                }
                                Err(e) => p.out_error = Some(e),
                            }
                        }
                    }
                    if ui.add_enabled(!out.is_empty(), egui::Button::new("Clear")).clicked() {
                        p.out_error = None;
                        reqs.push(Req::ClientOut(None));
                    }
                });
                if let Some(e) = &p.out_error {
                    ui.label(RichText::new(e).color(Tone::Error.colour(ui)).small());
                }
                ui.horizontal(|ui| {
                    ui.label("Code page of client names");
                    let now = shaiya_clientdata::wld_raw::Codepage::from_key(&s.client_codepage).unwrap_or_default();
                    egui::ComboBox::from_id_salt("client-codepage").selected_text(now.label()).show_ui(ui, |ui| {
                        for c in shaiya_clientdata::wld_raw::Codepage::ALL {
                            if ui.selectable_label(c == now, c.label()).clicked() {
                                s.client_codepage = c.key().to_string();
                            }
                        }
                    })
                    .response
                    .on_hover_text(PREF_CODEPAGE);
                });
            }
            Tab::Snap => {
                ui.label(PREF_SNAP);
                ui.add(egui::TextEdit::singleline(&mut p.snap_text).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        match parse_snap_steps(&p.snap_text) {
                            Ok(v) => {
                                s.snap_steps = v;
                                p.snap_error = None;
                                p.snap_text = snap_text(&s.snap_steps);
                            }
                            Err(e) => p.snap_error = Some(e),
                        }
                    }
                    if ui.button("Defaults").clicked() {
                        s.snap_steps = SNAP_STEPS_DEFAULT.to_vec();
                        p.snap_text = snap_text(&s.snap_steps);
                        p.snap_error = None;
                    }
                });
                if let Some(e) = &p.snap_error {
                    ui.label(RichText::new(e).color(Tone::Error.colour(ui)));
                }
            }
        }
    });
    if server_before != (s.server_data.clone(), s.afterglow_defs_path.clone(), s.target_server) {
        reqs.push(Req::ServerDataChanged);
    }
    (open, reqs)
}

/// Where Explorer should be pointed for "Open data folder".
pub fn reveal(dir: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
    }
}

/// Explorer with a file selected (`explorer /select,<file>`).
pub fn reveal_file(file: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", file.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = file;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_steps_parse_and_reject() {
        assert_eq!(parse_snap_steps("4, 1, 0.5, 1"), Ok(vec![0.5, 1.0, 4.0]));
        assert_eq!(parse_snap_steps("0.1 0.25"), Ok(vec![0.1, 0.25]));
        for bad in ["", "a", "0", "-1", "2000", "1,,x"] {
            assert!(parse_snap_steps(bad).is_err(), "{bad:?}");
        }
        assert!(parse_snap_steps(&(1..=17).map(|n| n.to_string()).collect::<Vec<_>>().join(",")).is_err());
        assert_eq!(snap_text(&SNAP_STEPS_DEFAULT), "0.1, 0.5, 1, 2, 4, 8, 16, 64");
        assert_eq!(parse_snap_steps(&snap_text(&SNAP_STEPS_DEFAULT)).unwrap(), SNAP_STEPS_DEFAULT.to_vec());
    }
}
