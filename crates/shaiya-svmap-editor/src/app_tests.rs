//! Tests of the whole window: the app is driven through `egui::Context::run_ui` with synthetic input (no window, no dialogs), the way
//! `view2d::tests::Harness` drives the viewport. Declared from `app.rs` so the tests can reach the app's private state.

use super::*;
use crate::scatter::{self, Region};
use crate::recovery;
use crate::testutil::TempDir;
use egui::{Event, Modifiers, Pos2, RawInput, Rect, Vec2};
use shaiya_svmap::Svmap;
use crate::settings::CompareAgainst;
use crate::folderdiff;
use crate::mapini::MapIni;
use c5_ui::{Done, IniAct};

/// An app in a headless egui context.
struct Harness {
    ctx: egui::Context,
    app: App,
    t: f64,
    /// The text of the last frame (every text shape).
    text: String,
    /// Everything the app put on the system clipboard in the last frame.
    copied: Vec<String>,
    /// The window size of the next frames.
    size: Vec2,
}

fn svmap_file(dir: &Path, id: u32, size: u32) -> PathBuf {
    let p = dir.join(format!("{id}.svmap"));
    std::fs::write(&p, shaiya_svmap::write(&Svmap::new(size))).unwrap();
    p
}

fn all_text(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn walk(s: &egui::epaint::Shape, out: &mut String) {
        match s {
            egui::epaint::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    for c in shapes {
        walk(&c.shape, &mut out);
    }
    out
}

impl Harness {
    fn new(data_dir: Option<&Path>, opts: StartOptions) -> Harness {
        let ctx = egui::Context::default();
        let app = App::new_with(&ctx, "test adapter".to_string(), opts, data_dir.map(|d| d.to_path_buf()));
        Harness { ctx, app, t: 0.0, text: String::new(), copied: Vec::new(), size: Vec2::new(1400.0, 800.0) }
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.t += 0.05;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), ..Default::default() };
        input.time = Some(self.t);
        input.events = events;
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.frame(ui));
        out.textures_delta.clear();
        self.text = all_text(&out.shapes);
        self.copied = out.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t.clone()) } else { None }).collect();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(Vec::new());
        }
    }

    fn key(&mut self, key: egui::Key, modifiers: Modifiers) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
    }

    fn type_text(&mut self, text: &str) {
        self.frame(vec![Event::Text(text.to_string())]);
    }

    /// Runs a frame and returns the centre of the first text drawn exactly as `needle`.
    fn text_pos(&mut self, needle: &str) -> Option<Pos2> {
        self.t += 0.05;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), ..Default::default() };
        input.time = Some(self.t);
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.frame(ui));
        out.textures_delta.clear();
        out.shapes.iter().find_map(|c| match &c.shape {
            egui::epaint::Shape::Text(t) if t.galley.text() == needle => Some(t.pos + t.galley.rect.center().to_vec2()),
            _ => None,
        })
    }
}

fn client_dir(root: &TempDir) -> PathBuf {
    let c = root.join("client");
    std::fs::create_dir_all(c.join("world")).unwrap();
    c
}

#[test]
fn wizard_flow() {
    let data = TempDir::new("wiz-data");
    let maps = TempDir::new("wiz-maps");
    svmap_file(maps.path(), 1, 64);
    svmap_file(maps.path(), 2, 64);
    let root = TempDir::new("wiz-client");
    let client = client_dir(&root);

    // first start: an empty data folder shows the wizard
    let mut h = Harness::new(Some(data.path()), StartOptions::default());
    assert!(h.app.wizard.is_some() && !h.app.settings.first_run_done);
    h.frames(2);
    assert!(h.text.contains("Welcome to the Shaiya SVMap Editor") && h.text.contains("The server's map files"), "{}", h.text);

    // set the paths as typing or Browse would, wait for the inspections, finish
    let w = h.app.wizard.as_mut().unwrap();
    w.set_path(0, &maps.path().display().to_string());
    w.set_path(1, &client.display().to_string());
    w.wait();
    assert!(w.server.done.as_ref().is_some_and(|s| s.valid()) && w.client.done.as_ref().is_some_and(|c| c.valid()));
    let outcome = w.outcome(false);
    h.app.wizard = None;
    let ctx = h.ctx.clone();
    h.app.on_wizard_event(WizardEvent::Finished(outcome), &ctx);
    assert!(h.app.settings.first_run_done);
    assert_eq!(h.app.folder.as_ref().map(|f| f.maps.len()), Some(2));
    assert_eq!(h.app.client_path.as_deref(), Some(client.as_path()));
    h.frames(2);
    h.app.flush_settings();
    drop(h);

    // second start: no wizard, the folder and client come back
    let h2 = Harness::new(Some(data.path()), StartOptions::default());
    assert!(h2.app.wizard.is_none() && h2.app.settings.first_run_done);
    assert_eq!(h2.app.folder.as_ref().map(|f| f.maps.len()), Some(2));
    assert!(h2.app.client.is_some());
}

#[test]
fn closing_the_wizard_any_way_counts_as_done_and_old_users_skip_it() {
    let data = TempDir::new("wiz-close");
    let mut h = Harness::new(Some(data.path()), StartOptions::default());
    assert!(h.app.wizard.is_some());
    let ctx = h.ctx.clone();
    h.app.wizard = None;
    h.app.on_wizard_event(WizardEvent::Closed, &ctx);
    assert!(h.app.settings.first_run_done && h.app.folder.is_none());
    h.app.flush_settings();
    assert!(!Harness::new(Some(data.path()), StartOptions::default()).app.wizard.is_some());

    // a settings file from an older build that already names a folder: no wizard, and it is marked as seen
    let old = TempDir::new("wiz-old");
    let maps = TempDir::new("wiz-old-maps");
    svmap_file(maps.path(), 3, 64);
    std::fs::write(old.join("settings.json"), format!("{{\"svmap_folder\": {:?}}}", maps.path().display().to_string())).unwrap();
    let h = Harness::new(Some(old.path()), StartOptions::default());
    assert!(h.app.wizard.is_none() && h.app.settings.first_run_done);
    // a folder on the command line: no wizard either
    let fresh = TempDir::new("wiz-cmdline");
    let h = Harness::new(Some(fresh.path()), StartOptions { folder: Some(maps.path().to_path_buf()), ..Default::default() });
    assert!(h.app.wizard.is_none() && h.app.settings.first_run_done);
    // --no-settings: never
    assert!(Harness::new(None, StartOptions::default()).app.wizard.is_none());
}

#[test]
fn goto_moves_view() {
    let dir = TempDir::new("goto");
    let f = svmap_file(dir.path(), 1, 2048);
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.frames(3);
    h.key(egui::Key::G, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.app.palette.as_ref().is_some_and(|p| p.mode == PalMode::GoTo), "Ctrl+G opens Go to");
    h.type_text("cell 3391102");
    h.frames(1);
    assert!(h.text.contains("cell 3391102 = x 1662.5 z 1655.5"), "the live preview: {}", h.text);
    h.key(egui::Key::Enter, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.palette.is_none(), "Enter closes it");
    let v = h.app.docs[0].view;
    assert!((v.cx - 1662.5).abs() <= 0.5 && (v.cz - 1655.5).abs() <= 0.5, "view centre {} {}", v.cx, v.cz);
    assert!(v.zoom >= 0.5);
    assert!(h.app.crosshair.is_some(), "a crosshair marks the target");

    // a bad target keeps the popup open and does nothing
    h.key(egui::Key::G, Modifiers::COMMAND);
    h.frames(2);
    h.type_text("99999 99999");
    h.frames(1);
    assert!(h.text.contains("outside the map"));
    h.key(egui::Key::Enter, Modifiers::NONE);
    h.frames(1);
    assert!(h.app.palette.is_some());
    assert!((h.app.docs[0].view.cx - 1662.5).abs() <= 0.5);
    // Esc closes it
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.frames(1);
    assert!(h.app.palette.is_none());
}

#[test]
fn goto_record_selects_it_and_bmove_text_pastes_back() {
    let dir = TempDir::new("goto-rec");
    let mut m = Svmap::new(256);
    m.npcs.push(shaiya_svmap::NpcEntry { kind: 1, type_id: 1, points: vec![shaiya_svmap::NpcPoint { pos: [100.0, 0.0, 120.0], dir: 0.0 }] });
    let f = dir.join("7.svmap");
    std::fs::write(&f, shaiya_svmap::write(&m)).unwrap();
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.frames(3);
    let (r, _) = h.app.resolve_goto("npc 0").unwrap();
    h.app.do_goto(r);
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::Npcs, 0)]);
    // what Copy as GM move command puts on the clipboard is accepted by Go to again
    let text = gm_move_text([100.0, 120.0], 7);
    assert_eq!(text, "/bmove 100.0 120.0 7");
    let (r, _) = h.app.resolve_goto(&text).unwrap();
    h.app.do_goto(r);
    assert!((h.app.docs[0].view.cx - 100.0).abs() < 0.01 && (h.app.docs[0].view.cz - 120.0).abs() < 0.01);
    assert_eq!(position_text([1662.25, 1655.34]), "1662.2 1655.3");
    // with the cursor outside the viewport the selection is used
    h.app.vp.hover = None;
    assert_eq!(h.app.position_for_copy(), Some([100.0, 120.0]));
    h.app.docs[0].clear_selection();
    h.app.docs[0].view.cx = 50.0;
    h.app.docs[0].view.cz = 60.0;
    assert_eq!(h.app.position_for_copy(), Some([50.0, 60.0]));
    h.app.vp.hover = Some([1.0, 2.0]);
    assert_eq!(h.app.position_for_copy(), Some([1.0, 2.0]));
}

#[test]
fn session_restores_tabs() {
    let data = TempDir::new("session-data");
    let maps = TempDir::new("session-maps");
    svmap_file(maps.path(), 1, 128);
    svmap_file(maps.path(), 35, 256);
    let opts = StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1, 35], ..Default::default() };
    let mut h = Harness::new(Some(data.path()), opts);
    h.frames(3);
    assert_eq!(h.app.docs.len(), 2);
    h.app.docs[0].view = crate::doc::View { cx: 10.0, cz: 20.0, zoom: 2.5, fitted: true };
    h.app.docs[1].view = crate::doc::View { cx: 111.0, cz: 222.0, zoom: 0.75, fitted: true };
    h.app.active = 1;
    h.app.tool = Tool::Box;
    h.app.flush_settings();
    drop(h);

    // the next start: the same two tabs, cameras, tool; map 35 is active
    let mut h = Harness::new(Some(data.path()), StartOptions::default());
    h.frames(2);
    assert_eq!(h.app.docs.len(), 2);
    assert_eq!(h.app.docs[0].map_id, Some(1));
    assert_eq!(h.app.docs[1].map_id, Some(35));
    assert_eq!((h.app.docs[0].view.cx, h.app.docs[0].view.cz, h.app.docs[0].view.zoom), (10.0, 20.0, 2.5));
    assert_eq!((h.app.docs[1].view.cx, h.app.docs[1].view.cz, h.app.docs[1].view.zoom), (111.0, 222.0, 0.75));
    assert_eq!(h.app.active, 1);
    assert_eq!(h.app.tool, Tool::Box);
    assert!(h.app.docs.iter().all(|d| d.view.fitted), "the restored camera is not replaced by a fit");
    assert!(h.app.toast.is_none());
    drop(h);

    // a map that was deleted: one toast, the other tab comes back
    std::fs::remove_file(maps.join("1.svmap")).unwrap();
    let mut h = Harness::new(Some(data.path()), StartOptions::default());
    h.frames(2);
    assert_eq!(h.app.docs.len(), 1);
    assert_eq!(h.app.docs[0].map_id, Some(35));
    assert_eq!(h.app.active, 0);
    let toast = h.app.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.contains("1 map from the last session is gone") && toast.contains("1.svmap"), "{toast}");
    drop(h);

    // Preferences can switch the restore off
    let mut h = Harness::new(Some(data.path()), StartOptions::default());
    h.app.settings.session.restore = false;
    h.app.flush_settings();
    drop(h);
    let h = Harness::new(Some(data.path()), StartOptions::default());
    assert!(h.app.docs.is_empty());
    // a command line map is never mixed with the session
    let h = Harness::new(Some(data.path()), StartOptions { files: vec![maps.join("35.svmap")], ..Default::default() });
    assert_eq!(h.app.docs.len(), 1);
}

#[test]
fn empty_states_show() {
    // no folder, no tab, no client
    let mut h = Harness::new(None, StartOptions::default());
    // the Maps tab and the Outliner are two tabs of the Browser; "Show both" puts them in one view
    h.app.settings.layout.browser_split = true;
    h.frames(3);
    for needle in ["Open map folder...", "Open file...", "Run setup...", "No folder open.", "No map open.", "client: none"] {
        assert!(h.text.contains(needle), "the empty window lacks {needle:?}:\n{}", h.text);
    }
    // a folder but no tab
    let dir = TempDir::new("empty-folder");
    svmap_file(dir.path(), 4, 64);
    let mut h = Harness::new(None, StartOptions { folder: Some(dir.path().to_path_buf()), ..Default::default() });
    h.frames(6);
    assert!(h.text.contains("Double-click a map, or press Ctrl+P."), "{}", h.text);
    assert!(!h.text.contains("Open map folder..."));
    // a client but no folder
    let root = TempDir::new("empty-client");
    let c = client_dir(&root);
    let mut h = Harness::new(None, StartOptions { client: Some(c), ..Default::default() });
    h.frames(3);
    assert!(h.text.contains("Client data loaded. Open the server map folder to start."), "{}", h.text);
    assert!(h.text.contains("client: 2 problems") && !h.text.contains("client: none"), "no Monster.SData or NpcQuest.SData in the test client: {}", h.text);
    // an open map with no records: the outliner says how to add them, the inspector what to click
    let f = svmap_file(dir.path(), 5, 64);
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.app.settings.layout.browser_split = true;
    h.app.open_dock(DockTab::Problems);
    h.frames(4);
    assert!(h.text.contains("(none) draw one with Box (2)") && h.text.contains("(none) place one with NPC (3)"), "{}", h.text);
    assert!(h.text.contains("Click a record. Drag on empty ground to select several. Ctrl+F filters the outliner."));
    assert!(h.text.contains("No problems found."), "{}", h.text);
    h.app.open_dock(DockTab::Changes);
    h.frames(2);
    assert!(h.text.contains("No edits since the file was opened or saved."), "{}", h.text);
    // hints off: the how-to texts go, the "(none)" stays
    h.app.settings.hints = false;
    h.frames(2);
    assert!(h.text.contains("(none)") && !h.text.contains("draw one with Box"));
    assert!(!h.text.contains("Click a record. Drag on empty ground"));
}

#[test]
fn recent_maps_are_buttons_in_the_empty_state() {
    let dir = TempDir::new("recent-buttons");
    for id in [4, 9] {
        svmap_file(dir.path(), id, 64);
    }
    let mut h = Harness::new(None, StartOptions { folder: Some(dir.path().to_path_buf()), ..Default::default() });
    h.app.settings.add_recent(&dir.join("4.svmap").display().to_string());
    h.app.settings.add_recent(&dir.join("9.svmap").display().to_string());
    h.frames(6);
    assert!(h.text.contains("Recent maps") && h.text.contains("9 Map 9 —") && h.text.contains("4 Map 4 —"), "{}", h.text);
}

#[test]
fn f1_opens_the_page_for_the_tool_and_the_selection() {
    let dir = TempDir::new("f1");
    let f = svmap_file(dir.path(), 1, 64);
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.frames(2);
    assert!(!h.app.help_open);
    h.key(egui::Key::F1, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.help_open);
    let page = |h: &Harness| help::guide().pages[h.app.help.page].title.clone();
    assert_eq!(page(&h), "Getting started");
    assert!(h.text.contains("Search"), "the window is drawn: {}", h.text);
    // the active tool decides
    h.app.tool = Tool::Portal;
    h.app.draw_target = DrawTarget::MobArea;
    h.app.run(Cmd::Help, &h.ctx.clone());
    assert_eq!(page(&h), "The sections");
    assert_eq!(h.app.help.scroll_to.as_deref(), Some("portals"));
    h.frames(3);
    assert!(h.app.help.scroll_to.is_none(), "the page scrolled to the heading and let go of it");
    // with the Select tool, the selected section does
    h.app.tool = Tool::Select;
    h.app.docs[0].map.regions.push(shaiya_svmap::Region { min: [1.0, 0.0, 1.0], max: [5.0, 2.0, 5.0], kind: 0, b: 0, c: 0, d: 0 });
    h.app.docs[0].select_only(SectionId::Regions, 0);
    h.app.run(Cmd::Help, &h.ctx.clone());
    assert_eq!(h.app.help.scroll_to.as_deref(), Some("regions"));
}

#[test]
fn the_question_mark_toggles_the_shortcut_sheet() {
    let mut h = Harness::new(None, StartOptions::default());
    h.frames(2);
    h.key(egui::Key::Questionmark, Modifiers::SHIFT);
    h.frames(2);
    assert!(h.app.shortcuts_open);
    assert!(h.text.contains("Ctrl+Shift+P") && h.text.contains("Ctrl+G"), "{}", h.text);
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.frames(2);
    assert!(!h.app.shortcuts_open);
}

#[test]
fn command_keys_run_commands_and_text_fields_keep_their_keys() {
    let dir = TempDir::new("keys");
    let f = svmap_file(dir.path(), 1, 64);
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.frames(2);
    // 2 selects the Box tool, Esc goes back
    h.key(egui::Key::Num2, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Box);
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Select);
    // Ctrl+Shift+P: the palette in command mode; typing in it does not trigger the tool keys
    h.key(egui::Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    assert!(h.app.palette.as_ref().is_some_and(|p| p.mode == PalMode::Commands));
    h.type_text("3");
    h.key(egui::Key::Num3, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Select, "digits typed into the palette are text, not tool keys");
    assert!(h.app.palette.as_ref().is_some_and(|p| p.query.contains('3')));
    // Ctrl+P switches the open popup to map search
    h.key(egui::Key::P, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.app.palette.as_ref().is_some_and(|p| p.mode == PalMode::Maps && p.query.is_empty()));
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.frames(1);
    assert!(h.app.palette.is_none());
    // Ctrl+, opens the preferences, Ctrl+W closes the tab
    h.key(egui::Key::Comma, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.app.prefs.is_some());
    assert!(h.text.contains("Preferences"));
    h.key(egui::Key::W, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.app.docs.is_empty(), "Ctrl+W closed the unmodified tab");
}

#[test]
fn quick_open_lists_the_folder_open_tabs_and_recent_files() {
    let dir = TempDir::new("quick");
    for id in [1, 35, 36] {
        svmap_file(dir.path(), id, 64);
    }
    let elsewhere = TempDir::new("quick-else");
    let other = svmap_file(elsewhere.path(), 77, 64);
    let mut h = Harness::new(None, StartOptions { folder: Some(dir.path().to_path_buf()), maps: vec![35], ..Default::default() });
    h.app.settings.add_recent(&other.display().to_string());
    h.frames(6);
    let entries = h.app.map_entries();
    let ids: Vec<Option<u32>> = entries.iter().map(|e| e.id).collect();
    assert_eq!(ids, vec![Some(1), Some(35), Some(36), Some(77)]);
    assert!(entries[1].open && !entries[0].open && entries[3].recent);
    let ranked = palette::rank_maps("35", &entries);
    assert_eq!(entries[ranked[0]].id, Some(35));
    // picking a map that is not open opens it, picking an open one switches to it
    let e77 = entries[3].clone();
    h.app.open_entry(&e77);
    assert_eq!(h.app.docs.len(), 2);
    assert_eq!(h.app.docs[h.app.active].map_id, Some(77));
    let e35 = entries[1].clone();
    h.app.open_entry(&e35);
    assert_eq!(h.app.docs[h.app.active].map_id, Some(35));
    assert_eq!(h.app.docs.len(), 2);
    // without a folder: open tabs and recent files only
    let mut h = Harness::new(None, StartOptions { files: vec![other.clone()], ..Default::default() });
    h.frames(3);
    assert_eq!(h.app.map_entries().len(), 1);
}

#[test]
fn drag_and_drop_classifies_paths() {
    let t = TempDir::new("drop");
    let maps = t.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    let m1 = svmap_file(&maps, 1, 64);
    let client = client_dir(&t);
    let game = t.join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("data.sah"), b"x").unwrap();
    std::fs::write(game.join("data.saf"), b"x").unwrap();
    let empty = t.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(t.join("notes.txt"), b"x").unwrap();
    assert_eq!(classify_drop(&m1), DropKind::Map);
    assert_eq!(classify_drop(&maps), DropKind::MapFolder);
    assert_eq!(classify_drop(&client), DropKind::Client(client.clone()));
    assert_eq!(classify_drop(&game), DropKind::Client(game.clone()));
    assert_eq!(classify_drop(&game.join("data.sah")), DropKind::Client(game.clone()));
    assert_eq!(classify_drop(&empty), DropKind::Unknown);
    assert_eq!(classify_drop(&t.join("notes.txt")), DropKind::Unknown);
    assert_eq!(classify_drop(&t.join("missing")), DropKind::Unknown);
    // dropping several things at once does each in order; a bad one gets a toast
    let mut h = Harness::new(None, StartOptions::default());
    let ctx = h.ctx.clone();
    h.app.handle_dropped(vec![client.clone(), maps.clone(), m1.clone()], &ctx);
    assert_eq!(h.app.client_path.as_deref(), Some(client.as_path()));
    assert_eq!(h.app.folder.as_ref().map(|f| f.maps.len()), Some(1));
    assert_eq!(h.app.docs.len(), 1);
    h.app.handle_dropped(vec![t.join("notes.txt")], &ctx);
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.contains("is not a map file")));
}

#[test]
fn recent_list_orders_pins_folders_maps_and_clears() {
    let t = TempDir::new("recent");
    let maps = t.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    let f1 = svmap_file(&maps, 1, 64).display().to_string();
    let f2 = svmap_file(&maps, 2, 64).display().to_string();
    let dir = maps.display().to_string();
    let gone = t.join("gone.svmap").display().to_string();
    let mut h = Harness::new(None, StartOptions::default());
    for p in [&f1, &gone, &dir, &f2] {
        h.app.settings.add_recent(p);
    }
    let order: Vec<String> = h.app.recent_entries().into_iter().map(|e| e.0).collect();
    assert_eq!(order, vec![dir.clone(), f2.clone(), gone.clone(), f1.clone()], "folders first, then maps in recent order");
    let exists: Vec<bool> = h.app.recent_entries().into_iter().map(|e| e.2).collect();
    assert_eq!(exists, vec![true, true, false, true], "a missing file is flagged");
    h.app.settings.toggle_pin(&f1);
    assert_eq!(h.app.recent_entries()[0].0, f1, "pinned first");
    h.app.settings.clear_unpinned_recent();
    assert_eq!(h.app.recent_entries().len(), 1);
    assert!(h.app.recent_label(&f1, false).starts_with("1 Map 1 — "));
}

#[test]
fn can_run_gives_a_reason_for_every_disabled_command() {
    let mut h = Harness::new(None, StartOptions::default());
    h.frames(2);
    for i in commands::all() {
        if let Err(why) = h.app.can_run(i.cmd) {
            assert!(!why.is_empty(), "{} is disabled without a reason", i.id);
        }
    }
    assert_eq!(h.app.can_run(Cmd::Save), Err("no map open"));
    assert_eq!(h.app.can_run(Cmd::Undo), Err("no map open").or(Err("nothing to undo")));
    assert!(h.app.can_run(Cmd::NewFromClient).unwrap_err().contains("needs the client data"));
    assert!(h.app.can_run(Cmd::OpenDataFolder).unwrap_err().contains("no data folder"));
    assert!(h.app.can_run(Cmd::OpenFile).is_ok() && h.app.can_run(Cmd::Palette).is_ok() && h.app.can_run(Cmd::Tool(Tool::Paint)).is_ok());
    // running a disabled command does nothing and does not panic
    h.app.run(Cmd::Save, &h.ctx.clone());
    h.app.run(Cmd::Delete, &h.ctx.clone());
    assert!(h.app.dialog.is_none());
}

#[test]
fn preferences_apply_scale_and_theme_and_reset_the_layout() {
    let data = TempDir::new("prefs");
    let mut h = Harness::new(Some(data.path()), StartOptions { files: vec![], ..Default::default() });
    h.app.wizard = None;
    h.frames(2);
    assert_eq!(h.ctx.zoom_factor(), 1.0);
    h.app.settings.ui_scale = 1.5;
    h.app.settings.theme = crate::settings::Theme::Light;
    h.frames(2);
    assert_eq!(h.ctx.zoom_factor(), 1.5);
    assert_eq!(h.ctx.options(|o| o.theme_preference), egui::ThemePreference::Light);
    h.app.settings.left_width = 500.0;
    h.app.settings.layers.grid = false;
    let gen = h.app.layout_gen;
    h.app.reset_layout();
    assert_eq!(h.app.settings.left_width, Settings::default().left_width);
    assert!(h.app.settings.layers.grid);
    assert_eq!(h.app.layout_gen, gen + 1);
    // the settings go to the data folder's settings.json
    h.app.flush_settings();
    let saved = Settings::load_from(&data.join("settings.json"));
    assert_eq!(saved.ui_scale, 1.5);
}

#[test]
fn the_log_line_for_a_panic_free_run_needs_no_data_folder() {
    // --no-settings: nothing is persisted, flush is a no-op
    let mut h = Harness::new(None, StartOptions::default());
    h.app.settings.first_run_done = true;
    h.app.flush_settings();
    assert!(!h.app.persist && h.app.settings_path.is_none());
}

#[test]
fn every_help_page_renders_and_search_highlights() {
    let mut h = Harness::new(None, StartOptions::default());
    h.app.help_open = true;
    let pages = help::guide().pages.len();
    for i in 0..pages {
        h.app.help.page = i;
        h.frames(2);
        let title = help::guide().pages[i].title.clone();
        assert!(h.text.contains(&title), "page {i} ({title}) is not drawn:\n{}", h.text);
    }
    // the generated pages carry their tables
    h.app.help.goto_named("Problems explained");
    h.frames(2);
    assert!(h.text.contains("pos-out-of-range"), "{}", h.text);
    h.app.help.goto_named("Keyboard shortcuts");
    h.frames(2);
    assert!(h.text.contains("Ctrl+O"), "{}", h.text);
    // a search narrows the page tree
    h.app.help.search = "bmove".to_string();
    h.frames(2);
    assert!(h.text.contains("Go to") || h.text.contains("Moving around"));
    h.app.help.search = "zzzqqq".to_string();
    h.frames(2);
    assert!(h.text.contains("No page has those words."));
}

// ------------------------------------------------------------------------------------------------------------------ safety features

fn add_area(d: &mut Doc, x: f32) {
    let n = d.map.mob_areas.len();
    let a = shaiya_svmap::MobArea { min: [x, 0.0, 10.0], max: [x + 4.0, 2.0, 14.0], mobs: vec![shaiya_svmap::MobEntry { mob: 3, count: 2 }] };
    d.commit("add area", vec![Op::insert(n, Rec::Mob(a))]);
}

fn one_map_app(data: &Path, maps: &Path) -> Harness {
    let mut h = Harness::new(Some(data), StartOptions { folder: Some(maps.to_path_buf()), maps: vec![1], ..Default::default() });
    h.frames(2);
    h
}

#[test]
fn recovery_write_scan_restore() {
    let data = TempDir::new("rec-app-data");
    let maps = TempDir::new("rec-app-maps");
    let file = svmap_file(maps.path(), 1, 64);
    let disk = std::fs::read(&file).unwrap();
    let mut h = one_map_app(data.path(), maps.path());
    assert_eq!(h.app.docs.len(), 1);
    h.app.settings.autosave_secs = 15;
    let uid = h.app.docs[0].uid;
    let key = recovery::entry_key(Some(&file), uid);
    let svmap = recovery::dir(data.path()).join(format!("{key}.svmap"));
    let json = recovery::dir(data.path()).join(format!("{key}.json"));

    // a clean tab writes nothing
    h.app.autosave_tick(true);
    assert!(!svmap.exists());
    // an edit, then the interval has not passed: still nothing; an edit that is not quiet yet waits too
    add_area(&mut h.app.docs[0], 20.0);
    h.app.safety.last_autosave = Some(Instant::now());
    h.app.autosave_tick(false);
    assert!(!svmap.exists(), "not due yet");
    // the interval passed and the edit is 3 s old: both files, the map byte-exact
    h.app.safety.last_autosave = Some(Instant::now() - Duration::from_secs(16));
    h.app.docs[0].edit_at = Instant::now() - Duration::from_secs(3);
    h.app.autosave_tick(false);
    assert!(svmap.is_file() && json.is_file());
    let edited = h.app.docs[0].map.clone();
    assert_eq!(std::fs::read(&svmap).unwrap(), shaiya_svmap::write(&edited));
    assert_eq!(std::fs::read(&file).unwrap(), disk, "the real file is never written by autosave");
    drop(h);

    // the next start: the entry has a fresh heartbeat, so it belongs to a running window and is not offered
    let h = one_map_app(data.path(), maps.path());
    assert!(h.app.safety.recovery.is_none());
    drop(h);

    // make it old (the editor that wrote it is gone): now it is offered
    let mut found = recovery::scan(data.path(), u64::MAX / 2, 1);
    assert_eq!(found.len(), 1);
    found[0].meta.heartbeat_unix_ms = 1;
    recovery::write_meta(data.path(), &key, &found[0].meta).unwrap();
    let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(maps.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    let mut w = h.app.safety.recovery.take().expect("the recovery dialog has the entry");
    assert_eq!(w.entries.len(), 1);
    h.app.restore_recovered(w.entries.remove(0)).unwrap();
    let d = &mut h.app.docs[0];
    assert!(d.is_dirty() && d.path.as_deref() == Some(file.as_path()));
    assert_eq!(d.map, edited);
    assert_eq!(d.undo.len(), 1);
    assert_eq!(d.undo[0].label, "recovered edits");
    // Undo gives the disk version, byte for byte
    d.undo();
    assert_eq!(shaiya_svmap::write(&d.map), disk);
    assert!(!d.is_dirty());
    d.redo();
    assert_eq!(d.map, edited);
    assert_eq!(std::fs::read(&file).unwrap(), disk, "nothing is written until the user saves");
    // the entry is still on disk until the save
    assert!(svmap.is_file());
    let uid = h.app.docs[0].uid;
    h.app.begin_save(uid, None, true);
    assert_eq!(std::fs::read(&file).unwrap(), shaiya_svmap::write(&edited));
    assert!(!svmap.exists() && !json.exists(), "a save removes the entry");
    assert!(recovery::scan(data.path(), u64::MAX / 2, 1).is_empty());
}

#[test]
fn recovery_skips_live_heartbeat() {
    let data = TempDir::new("rec-live-data");
    let maps = TempDir::new("rec-live-maps");
    let file = svmap_file(maps.path(), 1, 64);
    let mut h = one_map_app(data.path(), maps.path());
    h.app.settings.autosave_secs = 60;
    add_area(&mut h.app.docs[0], 30.0);
    h.app.docs[0].edit_at = Instant::now() - Duration::from_secs(5);
    h.app.autosave_tick(true);
    let key = recovery::entry_key(Some(&file), h.app.docs[0].uid);
    assert!(recovery::dir(data.path()).join(format!("{key}.json")).is_file());
    // a second window starting now sees a heartbeat younger than 2 x 60 s + 10 s
    let h2 = Harness::new(Some(data.path()), StartOptions { folder: Some(maps.path().to_path_buf()), ..Default::default() });
    assert!(h2.app.safety.recovery.is_none(), "another running window's entry is not offered");
    // a heartbeat tick keeps the entry fresh without rewriting the map
    let before = std::fs::read(recovery::dir(data.path()).join(format!("{key}.svmap"))).unwrap();
    h.app.autosave_tick(true);
    assert_eq!(std::fs::read(recovery::dir(data.path()).join(format!("{key}.svmap"))).unwrap(), before);
}

#[test]
fn recovery_leaves_entries_this_window_does_not_own() {
    // work an earlier session left under map 1's key, kept with "Later"; the same map is open in this window
    let data = TempDir::new("rec-own-data");
    let maps = TempDir::new("rec-own-maps");
    let file = svmap_file(maps.path(), 1, 64);
    let key = recovery::entry_key(Some(&file), 0);
    let mut later = shaiya_svmap::read(&std::fs::read(&file).unwrap()).unwrap();
    later.mob_areas.push(shaiya_svmap::MobArea { min: [40.0, 0.0, 40.0], max: [44.0, 2.0, 44.0], mobs: vec![shaiya_svmap::MobEntry { mob: 7, count: 1 }] });
    let meta = recovery::Meta { path: Some(file.display().to_string()), map_id: Some(1), title: "1.svmap".into(), steps: 1, heartbeat_unix_ms: 1, ..Default::default() };
    recovery::write_entry(data.path(), &key, &later, &meta).unwrap();
    let offered = || recovery::scan(data.path(), u64::MAX / 2, 1);
    let mut h = one_map_app(data.path(), maps.path());
    assert!(h.app.safety.recovery.take().is_some(), "offered; the user picks Later");
    // a clean tab of the same file is ticked, closed, and the window exits cleanly: the entry it does not own stays
    h.app.autosave_tick(true);
    h.app.recovery_remove_all();
    h.app.close_tab(0);
    assert_eq!(offered().len(), 1, "Later keeps the work");
    // the file is opened and edited: the old entry is parked, this tab writes its own; saving removes only its own
    h.app.open_file(&file);
    add_area(&mut h.app.docs[0], 20.0);
    h.app.autosave_tick(true);
    assert_eq!(offered().len(), 2);
    let uid = h.app.docs[0].uid;
    h.app.begin_save(uid, None, true);
    let left = offered();
    assert_eq!(left.len(), 1);
    assert_ne!(left[0].key, key, "the parked entry of the earlier session");
    assert_eq!(left[0].map.as_ref().unwrap(), &later);
    // restoring the parked entry: this tab takes it over under its own key once it writes one, and nothing is lost in between
    h.app.close_tab(0);
    h.app.restore_recovered(left.into_iter().next().unwrap()).unwrap();
    assert_eq!(offered().len(), 1);
    h.app.docs[0].edit_at = Instant::now() - Duration::from_secs(3);
    h.app.autosave_tick(true);
    let now = offered();
    assert_eq!(now.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec![key.as_str()], "the stale key is gone, the work is under the tab's own key");
    assert_eq!(now[0].map.as_ref().unwrap(), &later);
    // the tab is discarded: its entry goes
    h.app.close_tab(0);
    assert!(offered().is_empty());
}

// ------------------------------------------------------------------------------------------------------------------ templates and context menu

fn mob_area(x: f32, mobs: &[(u32, u32)]) -> Rec {
    Rec::Mob(shaiya_svmap::MobArea { min: [x, 0.0, 10.0], max: [x + 4.0, 2.0, 14.0], mobs: mobs.iter().map(|(m, c)| shaiya_svmap::MobEntry { mob: *m, count: *c }).collect() })
}

/// A folder of three small maps, each with two areas that hold monster 5 and one that does not.
fn three_maps(dir: &Path) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    for id in 1..=3u32 {
        let mut m = Svmap::new(64);
        m.mob_areas.push(match mob_area(0.0, &[(5, 2), (7, 1)]) {
            Rec::Mob(a) => a,
            _ => unreachable!(),
        });
        m.mob_areas.push(match mob_area(10.0, &[(7, 3)]) {
            Rec::Mob(a) => a,
            _ => unreachable!(),
        });
        m.mob_areas.push(match mob_area(20.0, &[(5, 1)]) {
            Rec::Mob(a) => a,
            _ => unreachable!(),
        });
        let bytes = shaiya_svmap::write(&m);
        std::fs::write(dir.join(format!("{id}.svmap")), &bytes).unwrap();
        out.push((id, bytes));
    }
    out
}

/// Renders frames until `cond` holds, polling between frames; panics with `what` after `limit`. Background work (folder scans, map
/// loads, thumbnails) finishes by itself, so a test waits for the event and never for a fixed time.
fn wait_until(h: &mut Harness, what: &str, limit: Duration, mut cond: impl FnMut(&mut Harness) -> bool) {
    let deadline = Instant::now() + limit;
    loop {
        h.frame(Vec::new());
        if cond(h) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out after {limit:?} waiting for {what}");
        // pacing of the poll only: the loop ends on the condition, not on this pause
        std::thread::sleep(Duration::from_millis(2));
    }
}

const WAIT: Duration = Duration::from_secs(60);

fn wait_for_folder(h: &mut Harness) {
    wait_until(h, "the map folder scan", WAIT, |h| h.app.folder.as_ref().is_some_and(|f| f.rx.is_none()));
    h.frames(2);
}

#[test]
fn replace_across_maps() {
    let maps = TempDir::new("replace-maps");
    let disk = three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), ..Default::default() });
    wait_for_folder(&mut h);
    assert!(h.app.docs.is_empty(), "no tab is open yet");
    h.app.q3.find.query = "mob:5".to_string();
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.len(), 6, "2 areas on each of 3 maps");
    // map 2 is only open read-only: it is skipped, not edited
    h.app.open_read_only(&maps.join("2.svmap"), None, Some(2));
    h.frames(2);
    let ro_uid = h.app.docs[0].uid;
    let summary = h.app.replace_monster(5, 6);
    assert!(summary.contains("in 4 areas on 2 maps") && summary.contains("Skipped: 2 (read-only tab)"), "{summary}");
    // one editable tab per edited map, one undo step each, only monster 5 changed
    let editable: Vec<&Doc> = h.app.docs.iter().filter(|d| !d.read_only).collect();
    assert_eq!(editable.len(), 2);
    for d in &editable {
        assert_eq!(d.undo.len(), 1, "one step for map {:?}", d.map_id);
        assert!(d.is_dirty());
        let m = &d.map.mob_areas;
        assert_eq!(m[0].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>(), vec![(6, 2), (7, 1)]);
        assert_eq!(m[1].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>(), vec![(7, 3)], "the other monsters are untouched");
        assert_eq!(m[2].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>(), vec![(6, 1)]);
    }
    let ro = h.app.docs.iter().find(|d| d.uid == ro_uid).unwrap();
    assert!(ro.undo.is_empty() && ro.map.mob_areas[0].mobs[0].mob == 5, "the read-only tab is untouched");
    // nothing was saved
    for (id, bytes) in &disk {
        assert_eq!(&std::fs::read(maps.join(&format!("{id}.svmap"))).unwrap(), bytes, "map {id} on disk");
    }
    // the search sees the live maps now: monster 5 is only on the folder copy of map 2 (read-only tab)
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.len(), 2, "{:?}", h.app.q3.find.results);
    // undo per map restores
    for d in h.app.docs.iter_mut().filter(|d| !d.read_only) {
        d.undo();
        assert_eq!(d.map.mob_areas[0].mobs[0].mob, 5);
        assert!(!d.is_dirty());
    }
    // replacing again on a tab that is open and editable edits that tab (no second tab)
    h.app.q3.find.query = "mob:5".to_string();
    h.app.run_find();
    let n_tabs = h.app.docs.len();
    h.app.replace_monster(5, 8);
    assert_eq!(h.app.docs.len(), n_tabs, "maps 1 and 3 are open and editable (edited in place, no new tab); map 2 is skipped again");
    assert!(h.app.docs.iter().filter(|d| !d.read_only).all(|d| d.map.mob_areas[0].mobs[0].mob == 8));
}

#[test]
fn clipboard_json_roundtrip() {
    let maps = TempDir::new("clip-maps");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1, 2], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_map_id(1);
    h.frames(2);
    // copy two records: the system clipboard gets the JSON
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 2)];
    h.frame(vec![Event::Copy]);
    assert_eq!(h.copied.len(), 1, "one copy per press: {:?}", h.copied);
    let text = h.copied[0].clone();
    assert!(text.starts_with("{\"svmap-editor\":1,\"map\":1,\"records\":["), "{text}");
    let want = vec![edit::get(&h.app.docs[0].map, SectionId::MobAreas, 0).unwrap(), edit::get(&h.app.docs[0].map, SectionId::MobAreas, 2).unwrap()];
    assert_eq!(edit::parse_clipboard(&text).unwrap(), want);
    // a key press that also arrives (another backend) does not copy twice
    h.frame(vec![Event::Key { key: egui::Key::C, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }, Event::Copy]);
    assert_eq!(h.copied.len(), 1);
    // paste it into map 2 (another tab, as another editor window would): the cursor at the first record's corner puts them back
    h.app.open_map_id(2);
    h.frames(2);
    let before = h.app.docs[1].map.mob_areas.len();
    h.app.vp.hover = Some([0.0, 10.0]);
    h.frame(vec![Event::Paste(text.clone())]);
    let d = &h.app.docs[1];
    assert_eq!(d.map.mob_areas.len(), before + 2);
    assert_eq!(edit::get(&d.map, SectionId::MobAreas, before).unwrap(), want[0]);
    assert_eq!(edit::get(&d.map, SectionId::MobAreas, before + 1).unwrap(), want[1], "equal records");
    assert_eq!(d.undo.len(), 1, "one undo step");
    assert_eq!(d.selection, vec![(SectionId::MobAreas, before), (SectionId::MobAreas, before + 1)]);
    // Ctrl+Shift+V pastes the app's clipboard exactly in place (the key arrives as a key press: Shift is not a clipboard shortcut)
    h.app.vp.hover = Some([55.0, 55.0]);
    h.frame(vec![Event::Key { key: egui::Key::V, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND | Modifiers::SHIFT }]);
    let d = &h.app.docs[1];
    assert_eq!(d.map.mob_areas.len(), before + 4);
    assert_eq!(edit::get(&d.map, SectionId::MobAreas, before + 2).unwrap(), want[0], "in place: same position as the copy");
    assert_eq!(d.undo.len(), 2);
    // other text is ignored with a toast
    h.frame(vec![Event::Paste("hello".to_string())]);
    assert_eq!(h.app.docs[1].map.mob_areas.len(), before + 4);
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.contains("not svmap records")));
    // a read-only tab refuses the paste
    h.app.docs[1].read_only = true;
    h.frame(vec![Event::Paste(text)]);
    assert_eq!(h.app.docs[1].map.mob_areas.len(), before + 4);
    // typing in a text field keeps Ctrl+V for the field
    h.app.docs[1].read_only = false;
    h.app.q3.find.focus = true;
    h.app.open_dock(DockTab::Find);
    h.frames(3);
    let n = h.app.docs[1].map.mob_areas.len();
    h.frame(vec![Event::Paste("mob:5".to_string())]);
    assert_eq!(h.app.docs[1].map.mob_areas.len(), n, "the Find box took the paste");
    assert_eq!(h.app.q3.find.query, "mob:5");
}

#[test]
fn select_all_similar_and_matching() {
    let maps = TempDir::new("select-maps");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.docs[0].map.npcs.push(shaiya_svmap::NpcEntry { kind: 1, type_id: 2, points: vec![shaiya_svmap::NpcPoint { pos: [5.0, 0.0, 5.0], dir: 0.0 }] });
    // nothing selected: every visible, unlocked section; locked sections stay out
    h.app.settings.locks.npcs = true;
    h.app.run(Cmd::SelectAll, &h.ctx.clone());
    assert_eq!(h.app.docs[0].selection.len(), 3, "the 3 areas, not the locked NPC");
    // a selection: only its section
    h.app.settings.locks.npcs = false;
    h.app.docs[0].selection = vec![(SectionId::Npcs, 0)];
    h.app.run(Cmd::SelectAll, &h.ctx.clone());
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::Npcs, 0)]);
    // similar: the areas with the same monster set as area 2 (only monster 5)
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 2)];
    h.app.run(Cmd::SelectSimilar, &h.ctx.clone());
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::MobAreas, 2)]);
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 1)];
    h.app.run(Cmd::SelectSimilar, &h.ctx.clone());
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::MobAreas, 1)]);
    // matching: Find scoped to this map, then Select all on this map
    h.app.run(Cmd::SelectMatching, &h.ctx.clone());
    assert!(h.app.q3.find.this_map && h.app.dock_showing(DockTab::Find));
    h.app.q3.find.query = "mob:5".to_string();
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.len(), 2);
    h.app.select_hits_here();
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 2)]);
    // the all-maps scope counts every map, "select all on this map" takes the ones on the open map
    h.app.q3.find.this_map = false;
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.len(), 6);
    h.app.docs[0].selection.clear();
    h.app.select_hits_here();
    assert_eq!(h.app.docs[0].selection.len(), 2);
    // inside view: a tiny view around the first area keeps only it
    h.app.q3.find.this_map = true;
    h.app.q3.find.inside_view = true;
    h.app.vp.size = egui::Vec2::new(100.0, 100.0);
    h.app.docs[0].view = crate::doc::View { cx: 2.0, cz: 12.0, zoom: 4.0, fitted: true };
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.iter().map(|x| x.index).collect::<Vec<_>>(), vec![0]);
}

#[test]
fn bookmarks_and_templates_go_through_the_prompt() {
    let maps = TempDir::new("prompt-maps");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.docs[0].view = crate::doc::View { cx: 30.5, cz: 20.5, zoom: 3.0, fitted: true };
    // Ctrl+B asks for a name (the default is the position when no region is there) and keeps the view
    h.key(egui::Key::B, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.app.q3.prompt.is_some() && h.text.contains("Add bookmark"), "{}", h.text);
    assert_eq!(h.app.q3.prompt.as_ref().unwrap().text, format!("{:.0} {:.0}", 30.5f32, 20.5f32), "the view centre, rounded");
    h.app.q3.prompt.as_mut().unwrap().text = "gate".to_string();
    let p = h.app.q3.prompt.take().unwrap();
    h.app.finish_prompt(p);
    let marks = h.app.bookmarks_here();
    assert_eq!(marks.len(), 1);
    assert_eq!((marks[0].name.as_str(), marks[0].x, marks[0].z, marks[0].zoom), ("gate", 30.5, 20.5, 3.0));
    // Go to @gate finds it
    h.app.docs[0].view.cx = 1.0;
    let (r, _) = h.app.resolve_goto("@gate").unwrap();
    h.app.do_goto(r);
    assert_eq!((h.app.docs[0].view.cx, h.app.docs[0].view.cz), (30.5, 20.5));
    // a template from area 0
    h.app.save_template_prompt(0);
    h.app.q3.prompt.as_mut().unwrap().text = "pair".to_string();
    let p = h.app.q3.prompt.take().unwrap();
    h.app.finish_prompt(p);
    let t = h.app.settings.templates.last().unwrap().clone();
    assert_eq!((t.name.as_str(), t.size, t.height, t.entries.clone()), ("pair", [4.0, 4.0], 2.0, vec![[5, 2], [7, 1]]));
    assert_eq!(h.app.q3.template.as_deref(), Some("pair"));
    // "add monster area here" stamps it centred at the snapped position
    h.app.settings.snap = 1.0;
    h.app.do_ctx_act(view2d::CtxAct::AddArea([40.2, 40.4]), &h.ctx.clone());
    let a = h.app.docs[0].map.mob_areas.last().unwrap().clone();
    assert_eq!((a.min[0], a.min[2], a.max[0], a.max[2]), (38.0, 38.0, 42.0, 42.0));
    assert_eq!(a.mobs.len(), 2);
    // a read-only tab refuses every edit entry of the menu
    h.app.docs[0].read_only = true;
    let n = h.app.docs[0].map.mob_areas.len();
    h.app.do_ctx_act(view2d::CtxAct::AddArea([10.0, 10.0]), &h.ctx.clone());
    h.app.do_ctx_act(view2d::CtxAct::AddNpc([10.0, 10.0]), &h.ctx.clone());
    assert_eq!((h.app.docs[0].map.mob_areas.len(), h.app.docs[0].map.npcs.len()), (n, 0));
}

#[test]
fn export_files_have_the_bom_and_the_rows() {
    let maps = TempDir::new("export-maps");
    three_maps(maps.path());
    let out = TempDir::new("export-out");
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    let p = out.join("areas.csv");
    h.app.export_section_csv(crate::export::Kind::Areas, &p).unwrap();
    let t = std::fs::read_to_string(&p).unwrap();
    assert!(t.starts_with('\u{feff}') && t.contains("\r\n"));
    assert_eq!(t.trim_start_matches('\u{feff}').split("\r\n").filter(|l| !l.is_empty()).count(), 1 + 4, "header and 4 entries");
    let n = h.app.export_all_csv(out.path()).unwrap();
    assert_eq!(n, 5);
    assert!(out.join("1-monster-areas.csv").is_file() && out.join("1-regions.csv").is_file());
    let f = out.join("folder.csv");
    assert_eq!(h.app.export_folder_areas_csv(&f).unwrap(), 3);
    let t = std::fs::read_to_string(&f).unwrap();
    assert!(t.starts_with("\u{feff}map,index,"));
    assert_eq!(t.trim_start_matches('\u{feff}').split("\r\n").filter(|l| !l.is_empty()).count(), 1 + 3 * 4);
    let j = out.join("1.json");
    h.app.export_json_to(&j).unwrap();
    assert_eq!(shaiya_svmap::from_json(&std::fs::read_to_string(&j).unwrap()).unwrap(), h.app.docs[0].map);
}

#[test]
fn portal_links_window_and_the_map_card_list_the_incoming_portals() {
    let maps = TempDir::new("links-maps");
    for (id, to) in [(1u32, 2u32), (2, 1), (3, 1)] {
        let mut m = Svmap::new(64);
        m.portals.push(shaiya_svmap::Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 0, level_max: 0, to_map: to, to_pos: [9.0, 0.0, 9.0] });
        std::fs::write(maps.join(&format!("{id}.svmap")), shaiya_svmap::write(&m)).unwrap();
    }
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    let l = h.app.links_now().unwrap();
    assert_eq!(l.links.len(), 3);
    assert_eq!(l.one_way, 1, "3 -> 1 has no way back");
    // the map card of map 1 lists the portals of maps 2 and 3
    let inc = h.app.incoming_portals();
    assert_eq!(inc.iter().map(|i| i.from).collect::<Vec<_>>(), vec![2, 3]);
    h.frames(2);
    assert!(h.text.contains("Portals leading here (2)"), "{}", h.text);
    // the window
    h.app.run(Cmd::PortalLinks, &h.ctx.clone());
    h.frames(2);
    assert!(h.text.contains("Portal links") && h.text.contains("3 portals on 3 maps"), "{}", h.text);
    // Go to destination opens the destination map and puts the view on the arrival point
    h.app.docs[0].select_only(SectionId::Portals, 0);
    h.app.go_to_dest(0);
    assert_eq!(h.app.docs[h.app.active].map_id, Some(2));
    assert_eq!((h.app.docs[h.app.active].view.cx, h.app.docs[h.app.active].view.cz), (9.0, 9.0));
    assert!(h.app.crosshair.is_some());
}

/// Drop destination to ground reads the destination map's heights from the client.
#[test]
fn drop_destination_to_ground() {
    let (Some(maps), Some(client)) = (std::env::var_os("SVMAP_ORIGINALS"), std::env::var_os("SHAIYA_CLIENT")) else {
        eprintln!("skipped: SVMAP_ORIGINALS and SHAIYA_CLIENT are not both set");
        return;
    };
    let mut h = Harness::new(None, StartOptions { folder: Some(PathBuf::from(maps)), client: Some(PathBuf::from(client)), maps: vec![0], ..Default::default() });
    wait_for_folder(&mut h);
    // a portal of map 0 that leads to another map
    let Some((i, p)) = h.app.docs[0].map.portals.iter().copied().enumerate().find(|(_, p)| p.to_map != 0 && h.app.folder.as_ref().unwrap().maps.contains_key(&p.to_map)) else {
        eprintln!("skipped: map 0 has no portal to another map of the folder");
        return;
    };
    let ctx = h.ctx.clone();
    h.app.drop_dest_to_ground(i, &ctx);
    wait_until(&mut h, "the destination map of the drop", WAIT, |h| h.app.q3.drops.is_empty());
    println!("drop destination: portal {i} of map 0 to map {} ({:.1}, {:.1}); y {} -> {}", p.to_map, p.to_pos[0], p.to_pos[2], p.to_pos[1], h.app.docs[0].map.portals[i].to_pos[1]);
    let ground = match h.app.slots.get(&p.to_map) {
        Some(Slot::Ready(r)) => r.assets.ground.height(p.to_pos[0], p.to_pos[2]),
        _ => None,
    };
    let d = &h.app.docs[0];
    let after = d.map.portals[i];
    match ground {
        Some(g) => {
            assert_eq!(after.to_pos[1], g, "y is the destination map's ground");
            assert_eq!(after.to_pos[0], p.to_pos[0]);
            assert_eq!(after.to_pos[2], p.to_pos[2]);
            assert!(d.undo.len() <= 1, "one undo step (none when y was already the ground)");
        }
        None => assert_eq!(after, p, "no ground there: nothing changes"),
    }
}

#[test]
fn level_filter_and_overview_settings_are_kept() {
    let data = TempDir::new("q3-settings");
    let mut h = Harness::new(Some(data.path()), StartOptions::default());
    h.app.settings.level_filter = Some([10, 20]);
    h.app.settings.overview = false;
    h.app.settings.templates.push(crate::settings::Template { name: "t".into(), size: [3.0, 4.0], height: 2.0, entries: vec![[1, 2]] });
    h.app.flush_settings();
    drop(h);
    let h2 = Harness::new(Some(data.path()), StartOptions::default());
    assert_eq!(h2.app.settings.level_filter, Some([10, 20]));
    assert!(!h2.app.settings.overview);
    assert_eq!(h2.app.settings.templates.len(), 1);
}

// ------------------------------------------------------------------------------------------------------------------ templates regressions

/// "This map" results belong to the tab they were found on: switching tabs runs the search again, so Select all on this map and
/// the hover outline never use indices of another map.
#[test]
fn find_this_map_follows_the_active_tab() {
    let maps = TempDir::new("find-tabs");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1, 2], ..Default::default() });
    wait_for_folder(&mut h);
    assert_eq!(h.app.docs.len(), 2);
    // map 2 gets a fourth area with monster 5
    let extra = match mob_area(40.0, &[(5, 1)]) {
        Rec::Mob(a) => a,
        _ => unreachable!(),
    };
    h.app.docs[1].map.mob_areas.push(extra);
    h.app.switch_to(0);
    h.app.q3.find.this_map = true;
    h.app.q3.find.query = "mob:5".to_string();
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.iter().map(|x| x.index).collect::<Vec<_>>(), vec![0, 2]);
    h.app.switch_to(1);
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.iter().map(|x| (x.map, x.index)).collect::<Vec<_>>(), vec![(Some(2), 0), (Some(2), 2), (Some(2), 3)], "the results of the tab now shown");
    h.app.select_hits_here();
    assert_eq!(h.app.docs[1].selection.len(), 3);
}

/// Replace across maps on a tab with unsaved work keeps that work below the replace step, and an edit made after the search
/// (which moves the indices) is taken into account.
#[test]
fn replace_across_maps_with_dirty_tabs_and_moved_indices() {
    let maps = TempDir::new("replace-dirty");
    let disk = three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.q3.find.query = "mob:5".to_string();
    h.app.run_find();
    assert_eq!(h.app.q3.find.results.len(), 6);
    // after the search map 1 loses its first area (unsaved): area 2 is now index 1, and index 2 no longer exists
    let first = edit::get(&h.app.docs[0].map, SectionId::MobAreas, 0).unwrap();
    h.app.docs[0].commit("delete", vec![Op::remove(0, first)]);
    let summary = h.app.replace_monster(5, 6);
    assert!(summary.contains("in 5 areas on 3 maps"), "{summary}");
    let d = h.app.docs.iter().find(|d| d.map_id == Some(1)).unwrap();
    assert_eq!(d.undo.len(), 2, "the unsaved delete and the replace");
    assert_eq!(d.map.mob_areas.len(), 2);
    assert_eq!(d.map.mob_areas[0].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>(), vec![(7, 3)], "untouched");
    assert_eq!(d.map.mob_areas[1].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>(), vec![(6, 1)]);
    assert!(h.app.docs.iter().all(|d| d.map.mob_areas.iter().all(|a| a.mobs.iter().all(|e| e.mob != 5))));
    for (id, bytes) in &disk {
        assert_eq!(&std::fs::read(maps.join(&format!("{id}.svmap"))).unwrap(), bytes, "map {id} is not saved");
    }
    // undo on map 1 takes back only the replace
    let d = h.app.docs.iter_mut().find(|d| d.map_id == Some(1)).unwrap();
    d.undo();
    assert_eq!(d.map.mob_areas[1].mobs[0].mob, 5);
    assert_eq!(d.map.mob_areas.len(), 2, "the delete stays");
}

/// A read-only tab: a paste of portals asks nothing and leaves the map, the history and the selection alone; the context menu's
/// add entries leave no selection of a record that does not exist.
#[test]
fn read_only_paste_asks_nothing_and_keeps_the_selection() {
    let maps = TempDir::new("ro-paste");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_read_only(&maps.join("1.svmap"), None, Some(1));
    h.frames(2);
    let portal = shaiya_svmap::Portal { pos: [1.0, 0.0, 1.0], country: 0, level_min: 0, level_max: 0, to_map: 2, to_pos: [3.0, 0.0, 3.0] };
    let text = edit::clipboard_json(Some(2), &[Rec::Portal(portal), mob_area(0.0, &[(1, 1)])]).unwrap();
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 1)];
    let before = h.app.docs[0].map.clone();
    h.app.vp.hover = Some([20.0, 20.0]);
    h.frame(vec![Event::Paste(text)]);
    h.frames(2);
    assert!(h.app.dialog.is_none(), "no confirmation for an edit that cannot happen");
    let d = &h.app.docs[0];
    assert_eq!(d.map, before);
    assert!(d.undo.is_empty());
    assert_eq!(d.selection, vec![(SectionId::MobAreas, 1)]);
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.contains("read-only")), "{:?}", h.app.toast);
    h.app.do_ctx_act(view2d::CtxAct::AddArea([10.0, 10.0]), &h.ctx.clone());
    h.app.do_ctx_act(view2d::CtxAct::AddNpc([10.0, 10.0]), &h.ctx.clone());
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::MobAreas, 1)]);
}

/// Exports never write over a map file, whatever name the save dialog returned.
#[test]
fn exports_refuse_map_files() {
    let maps = TempDir::new("export-guard");
    let disk = three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    let target = maps.join("2.svmap");
    assert!(h.app.export_json_to(&target).is_err());
    assert!(h.app.export_section_csv(crate::export::Kind::Areas, &target).is_err());
    assert!(h.app.export_folder_areas_csv(&maps.join("3.SVMAP")).is_err());
    assert!(h.app.export_write(&maps.join("data.sah"), b"x").is_err());
    for (id, bytes) in &disk {
        assert_eq!(&std::fs::read(maps.join(&format!("{id}.svmap"))).unwrap(), bytes);
    }
    assert!(!maps.join("data.sah").exists());
}

/// Exports (CSV, JSON, PNG bytes, all-CSV) into the opened client folder, or under a program name, write nothing.
#[test]
fn exports_refuse_the_client_folder_and_program_names() {
    let maps = TempDir::new("export-guard-client-maps");
    three_maps(maps.path());
    let root = TempDir::new("export-guard-client");
    let c = client_dir(&root);
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), client: Some(c.clone()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    let before: Vec<_> = std::fs::read_dir(&c).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(h.app.export_section_csv(crate::export::Kind::Areas, &c.join("areas.csv")).is_err());
    assert!(h.app.export_json_to(&c.join("1.json")).is_err());
    assert!(h.app.export_folder_areas_csv(&c.join("folder.csv")).is_err());
    assert!(h.app.export_write(&c.join("view.png"), b"x").is_err());
    assert!(h.app.export_all_csv(&c).is_err());
    let sub = c.join("data");
    std::fs::create_dir_all(&sub).unwrap();
    assert!(h.app.export_write(&sub.join("view.png"), b"x").is_err());
    std::fs::remove_dir(&sub).unwrap();
    let after: Vec<_> = std::fs::read_dir(&c).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(before, after, "nothing was written into the client folder");
    // a program name is refused anywhere
    let out = TempDir::new("export-guard-client-out");
    assert!(h.app.export_write(&out.join("game.exe"), b"x").is_err());
    assert!(h.app.export_write(&out.join("D3D9.DLL"), b"x").is_err());
    assert!(!out.join("game.exe").exists() && !out.join("D3D9.DLL").exists());
    h.app.export_write(&out.join("ok.csv"), b"x").unwrap();
}

// ---------------------------------------------------------------------------------------------------------------------- layout

impl Harness {
    /// A click at a point: move there, press, release (one frame each).
    fn click(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.frame(vec![]);
    }
}

/// A one-map app with its own data folder (the layout settings are written there).
fn layout_app(data: &Path, maps: &Path) -> Harness {
    let f = svmap_file(maps, 1, 64);
    let mut h = Harness::new(Some(data), StartOptions { files: vec![f], ..Default::default() });
    h.frames(4);
    h
}

#[test]
fn default_layout_is_simple() {
    let data = TempDir::new("r1-default-data");
    let maps = TempDir::new("r1-default-maps");
    let h = layout_app(data.path(), maps.path());
    let l = &h.app.settings.layout;
    assert_eq!(h.app.settings.layout_name, "Simple");
    assert!(l.browser.visible && !l.browser.collapsed && l.inspector.visible && !l.dock.visible);
    assert_eq!(l.browser_tab, crate::layout::BrowserTab::Maps);
    assert_eq!((l.browser.side, l.inspector.side), (crate::layout::Side::Left, crate::layout::Side::Right));
    // what is on screen: the Maps tab, the Inspector, the tool palette with 9 tools and one toolbar row; no Dock, no frame time
    assert!(h.text.contains("Maps") && h.text.contains("Inspector"), "{}", h.text);
    assert!(!h.text.contains("Problems") && !h.text.contains("ms/frame"), "{}", h.text);
    assert_eq!(h.app.r1.drawn_palette, Some(9));
    assert!(h.app.r1.drawn_toolbar.is_some_and(|r| r.height() <= 34.0), "{:?}", h.app.r1.drawn_toolbar);
    assert!(h.app.r1.drawn_status);
    assert!(!h.app.settings.show_frame_time && !h.app.settings.show_3d_stats);
}

#[test]
fn layouts_save_and_switch() {
    let data = TempDir::new("r1-layouts-data");
    let maps = TempDir::new("r1-layouts-maps");
    let mut h = layout_app(data.path(), maps.path());
    // a layout of my own: Browser on the right, Inspector hidden, a wider Browser
    h.app.settings.layout.move_side(PanelId::Browser);
    h.app.settings.layout.hide(PanelId::Inspector);
    h.app.settings.layout.browser.size = 300.0;
    h.app.layout_gen += 1; // new panel ids: the remembered widths are forgotten
    h.app.settings.layout.browser_tab = crate::layout::BrowserTab::Outliner;
    h.frames(2);
    assert_eq!(h.app.save_layout_named("Simple"), Err("Pick another name."));
    assert_eq!(h.app.save_layout_named("review"), Err("Pick another name."));
    h.app.save_layout_named("Mine").unwrap();
    let mine = h.app.settings.layout.clone();
    assert_eq!(h.app.settings.layout_name, "Mine");
    assert!(!h.app.layout_changed());
    // Review: the Dock is open on Problems
    h.app.apply_layout("Review");
    h.frames(3);
    assert!(h.app.dock_showing(DockTab::Problems));
    assert!(h.text.contains("Problems"), "{}", h.text);
    // back to Mine: visibility, sides, tabs and sizes are restored
    h.app.apply_layout("Mine");
    h.frames(3);
    let l = &h.app.settings.layout;
    assert!(l.same_arrangement(&mine), "{l:?} vs {mine:?}");
    assert!((l.browser.size - 300.0).abs() <= 2.0, "size {}", l.browser.size);
    assert!(!l.inspector.visible && !l.dock.visible && l.browser.side == crate::layout::Side::Right);
    // a changed panel marks the layout
    h.app.settings.layout.show(PanelId::Inspector);
    assert!(h.app.layout_changed());
    h.app.apply_layout("Mine");
    assert!(!h.app.layout_changed() && !h.app.settings.layout.inspector.visible);
    // persisted: a restart shows it, and the saved list has it
    h.app.flush_settings();
    drop(h);
    let h2 = Harness::new(Some(data.path()), StartOptions::default());
    assert_eq!(h2.app.settings.layout_name, "Mine");
    assert!(h2.app.settings.layout.same_arrangement(&mine));
    assert_eq!(h2.app.settings.layouts.len(), 1);
    assert_eq!(h2.app.settings.layouts[0].name, "Mine");
}

#[test]
fn dock_opens_on_demand() {
    let data = TempDir::new("r1-dock-data");
    let maps = TempDir::new("r1-dock-maps");
    let mut h = layout_app(data.path(), maps.path());
    assert!(!h.app.settings.layout.dock.visible);
    // Ctrl+Shift+F opens the Dock on Find
    h.key(egui::Key::F, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    assert!(h.app.dock_showing(DockTab::Find), "{:?}", h.app.settings.layout);
    assert!(h.text.contains("all maps") || h.text.contains("this map"), "{}", h.text);
    // hide it, then the problems chip opens Problems; a second click collapses the Dock
    h.app.settings.layout.hide(PanelId::Dock);
    h.frames(2);
    assert!(!h.text.contains("Find"), "{}", h.text);
    let chip = h.app.r1.chip_rect.expect("the toolbar shows the problems chip when a map is open");
    h.click(chip.center());
    assert!(h.app.dock_showing(DockTab::Problems), "{:?}", h.app.settings.layout);
    let chip = h.app.r1.chip_rect.unwrap();
    h.click(chip.center());
    assert!(h.app.settings.layout.dock.collapsed && h.app.settings.layout.dock.visible, "{:?}", h.app.settings.layout.dock);
    // a collapsed Dock shows its tab names with counts, and a click on one opens it
    h.frames(2);
    assert!(h.text.contains("Problems 0 · 0") && h.text.contains("Changes (0)"), "{}", h.text);
    // the Changes tab opens it again when a command wants it
    h.app.open_dock(DockTab::Changes);
    assert!(h.app.dock_showing(DockTab::Changes));
}

#[test]
fn panel_collapse_and_side() {
    let data = TempDir::new("r1-panels-data");
    let maps = TempDir::new("r1-panels-maps");
    let mut h = layout_app(data.path(), maps.path());
    // collapse the Inspector: a rail of at most 32 px at the right
    h.app.settings.layout.set_collapsed(PanelId::Inspector, true);
    h.frames(3);
    let rail = h.app.r1.rails.iter().find(|(s, _)| *s == crate::layout::Side::Right).map(|(_, r)| *r).expect("a rail at the right");
    assert!(rail.width() <= 32.0, "rail {rail:?}");
    assert!(!h.text.contains("Inspector"), "the collapsed panel is not drawn: {}", h.text);
    // a click on its icon expands it
    h.click(rail.min + Vec2::new(14.0, 14.0));
    assert!(h.app.settings.layout.is_open(PanelId::Inspector), "{:?}", h.app.settings.layout.inspector);
    assert!(h.app.r1.rails.iter().all(|(s, _)| *s != crate::layout::Side::Right));
    // the Maps tab icon of a collapsed Browser opens the Browser on the Outliner tab
    h.app.settings.layout.set_collapsed(PanelId::Browser, true);
    h.frames(3);
    let rail = h.app.r1.rails.iter().find(|(s, _)| *s == crate::layout::Side::Left).map(|(_, r)| *r).expect("a rail at the left");
    h.click(rail.min + Vec2::new(14.0, 14.0 + 28.0));
    assert!(h.app.settings.layout.is_open(PanelId::Browser) && h.app.settings.layout.browser_tab == crate::layout::BrowserTab::Outliner);
    // the Browser moves to the right: stacked with the Inspector, the ratio is kept
    h.app.settings.layout.stack_ratio = 0.3;
    h.app.settings.layout.move_side(PanelId::Browser);
    h.frames(3);
    assert_eq!(h.app.settings.layout.stacked(crate::layout::Side::Right), vec![PanelId::Browser, PanelId::Inspector]);
    assert!(h.app.settings.layout.stacked(crate::layout::Side::Left).is_empty());
    assert_eq!(h.app.settings.layout.stack_ratio, 0.3);
    assert!(h.text.contains("Outliner") && h.text.contains("Inspector"), "{}", h.text);
}

#[test]
fn viewport_only_toggle() {
    let data = TempDir::new("r1-vo-data");
    let maps = TempDir::new("r1-vo-maps");
    let mut h = layout_app(data.path(), maps.path());
    h.app.settings.layout.open_dock(DockTab::Find);
    h.app.settings.layout.browser.size = 277.0;
    h.frames(2);
    let before = h.app.settings.layout.clone();
    h.key(egui::Key::F11, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.r1.vo.active());
    assert!(PanelId::ALL.iter().all(|p| !h.app.settings.layout.panel(*p).visible));
    assert!(h.app.r1.drawn_toolbar.is_none() && !h.app.r1.drawn_status && h.app.r1.drawn_palette.is_none());
    assert!(!h.text.contains("Inspector") && !h.text.contains("Snap"), "{}", h.text);
    // nothing is saved: the file still has the stored layout
    h.app.flush_settings();
    let saved = Settings::load_from(&data.join("settings.json"));
    assert_eq!(saved.layout, before);
    // F11 again restores exactly
    h.key(egui::Key::F11, Modifiers::NONE);
    h.frames(2);
    assert!(!h.app.r1.vo.active());
    assert_eq!(h.app.settings.layout, before);
    assert!(h.app.r1.drawn_toolbar.is_some() && h.app.r1.drawn_status && h.app.r1.drawn_palette == Some(9));
    // Esc also restores when no tool action is pending
    h.key(egui::Key::F11, Modifiers::NONE);
    h.frames(2);
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.frames(2);
    assert!(!h.app.r1.vo.active());
}

#[test]
fn toolbar_single_row() {
    let data = TempDir::new("r1-toolbar-data");
    let maps = TempDir::new("r1-toolbar-maps");
    let mut h = layout_app(data.path(), maps.path());
    h.size = Vec2::new(1280.0, 800.0);
    for t in Tool::ALL {
        h.app.tool = t;
        h.frames(3);
        let r = h.app.r1.drawn_toolbar.unwrap();
        assert!(r.height() <= 34.0, "{t:?} at 1280 px: toolbar {r:?}");
    }
    // at 900 px the options of the wide tools sit in the overflow popover, and the row is still one row
    h.size = Vec2::new(900.0, 800.0);
    for t in [Tool::Box, Tool::Paint] {
        h.app.tool = t;
        h.frames(3);
        assert!(h.app.r1.toolbar_overflow, "{t:?} at 900 px");
        assert!(h.app.r1.drawn_toolbar.unwrap().height() <= 34.0);
    }
    // a tool with no options never overflows
    h.app.tool = Tool::Select;
    h.frames(3);
    assert!(!h.app.r1.toolbar_overflow);
    // at 1400 px the Box options are inline
    h.size = Vec2::new(1400.0, 800.0);
    h.app.tool = Tool::Box;
    h.frames(3);
    assert!(!h.app.r1.toolbar_overflow && h.text.contains("template"), "{}", h.text);
}

/// Four layout edge cases: oversized saved panels, deleting the layout in use, Esc in Save layout, a very
/// narrow toolbar.
#[test]
fn r1_review_fixes() {
    let data = TempDir::new("r1-review-data");
    let maps = TempDir::new("r1-review-maps");
    let mut h = layout_app(data.path(), maps.path());
    // a panel size saved on a bigger screen starts at a share of this window, and the viewport keeps its room
    h.app.settings.layout.browser.size = 5000.0;
    h.app.settings.layout.inspector.size = 5000.0;
    h.app.settings.layout.open_dock(DockTab::Problems);
    h.app.settings.layout.dock.size = 5000.0;
    h.app.layout_gen += 1;
    h.frames(3);
    let l = &h.app.settings.layout;
    assert!(l.browser.size <= 0.4 * 1400.0 + 1.0 && l.inspector.size <= 0.4 * 1400.0 + 1.0, "{l:?}");
    assert!(l.dock.size <= 0.6 * 800.0 + 1.0, "{:?}", l.dock);
    assert!(h.text.contains("Inspector") && h.text.contains("Maps"), "{}", h.text);
    // deleting the layout in use switches to Simple, and the menu does not call it changed
    h.app.apply_layout("Review");
    h.app.save_layout_named("Mine").unwrap();
    h.app.delete_layout("Mine");
    assert_eq!(h.app.settings.layout_name, "Simple");
    assert!(h.app.settings.layouts.is_empty() && !h.app.layout_changed());
    assert_eq!(h.app.settings.layout, crate::layout::LayoutState::simple());
    // Esc in Save layout cancels the prompt only: viewport only and the tool stay as they are
    h.key(egui::Key::F11, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.r1.vo.active());
    h.app.tool = Tool::Box;
    h.app.r1.save_prompt = Some(("x".to_string(), None));
    h.frames(2);
    h.key(egui::Key::Escape, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.r1.save_prompt.is_none(), "Esc cancels Save layout");
    assert!(h.app.r1.vo.active() && h.app.tool == Tool::Box, "the same Esc reached the window");
    h.key(egui::Key::F11, Modifiers::NONE);
    h.frames(2);
    assert!(!h.app.r1.vo.active());
    // a window too narrow for the chips: they move into the "..." popover and the toolbar stays one row
    h.size = Vec2::new(330.0, 600.0);
    h.app.tool = Tool::Select;
    h.frames(3);
    assert!(h.app.r1.chip_rect.is_none() && h.text.contains("..."), "{}", h.text);
    assert!(h.app.r1.drawn_toolbar.unwrap().height() <= 34.0);
    h.size = Vec2::new(1400.0, 800.0);
    h.frames(3);
    assert!(h.app.r1.chip_rect.is_some());
}

#[test]
fn inspector_more_closed_by_default() {
    let data = TempDir::new("r1-more-data");
    let maps = TempDir::new("r1-more-maps");
    let f = maps.join("1.svmap");
    let mut m = Svmap::new(64);
    m.regions.push(shaiya_svmap::Region { min: [1.0, 0.0, 1.0], max: [9.0, 2.0, 9.0], kind: 0, b: 2, c: 3, d: 100 });
    std::fs::write(&f, shaiya_svmap::write(&m)).unwrap();
    let mut h = Harness::new(Some(data.path()), StartOptions { files: vec![f], ..Default::default() });
    h.frames(3);
    h.app.docs[0].select_only(SectionId::Regions, 0);
    h.frames(3);
    let has_line = |h: &Harness, s: &str| h.text.lines().any(|l| l == s);
    assert!(h.text.contains("More") && h.text.contains("Height"), "{}", h.text);
    assert!(!has_line(&h, "c") && !has_line(&h, "d") && !h.text.contains("bottom y"), "b, c, d and the heights are not drawn until opened:\n{}", h.text);
    // a click on the header opens the section and the state is kept in the settings
    h.app.settings.open_sections.insert("more-region".to_string(), true);
    h.frames(3);
    assert!(has_line(&h, "c") && has_line(&h, "d"), "{}", h.text);
    h.app.flush_settings();
    drop(h);
    let s = Settings::load_from(&data.join("settings.json"));
    assert_eq!(s.open_sections.get("more-region"), Some(&true));
}

#[test]
fn template_and_slab_reject_negative() {
    // template sizes: width and depth are at least half a unit, the height is not below 0
    let t = crate::settings::Template { name: "x".into(), size: [-5.0, 0.0], height: -3.0, entries: vec![[1, 2]] };
    let a = t.area_at(10.0, 0.0, 10.0);
    assert!(a.max[0] - a.min[0] >= 0.5 && a.max[2] - a.min[2] >= 0.5 && a.max[1] >= a.min[1], "{a:?}");
    let c = t.clamped();
    assert_eq!(c.size, [0.5, 0.5]);
    assert_eq!(c.height, 0.0);
    let nan = crate::settings::Template { size: [f32::NAN, f32::INFINITY], height: f32::NAN, ..Default::default() }.clamped();
    assert!(nan.size.iter().all(|v| v.is_finite()) && nan.height.is_finite());
    // a settings file with a negative template is repaired on load
    let dir = TempDir::new("r1-neg-template");
    std::fs::write(dir.join("settings.json"), r#"{"templates": [{"name": "bad", "size": [-4.0, -2.0], "height": -1.0, "entries": []}]}"#).unwrap();
    let s = Settings::load_from(&dir.join("settings.json"));
    assert_eq!((s.templates[0].size, s.templates[0].height), ([0.5, 0.5], 0.0));
    // the batch slab height
    let mut area = shaiya_svmap::MobArea { min: [0.0, 5.0, 0.0], max: [4.0, 7.0, 4.0], mobs: vec![] };
    crate::edit::mob_set_height(&mut area, -3.0);
    assert_eq!((area.min[1], area.max[1]), (5.0, 5.0));
    crate::edit::mob_set_height(&mut area, 2.0);
    assert_eq!((area.min[1], area.max[1]), (5.0, 7.0));
    crate::edit::mob_set_height(&mut area, f32::NAN);
    assert!(area.max[1].is_finite());
}

#[test]
fn bookmark_rename_follows_its_map() {
    let dir = TempDir::new("r1-bookmark");
    let data = TempDir::new("r1-bookmark-data");
    svmap_file(dir.path(), 1, 64);
    svmap_file(dir.path(), 2, 64);
    let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(dir.path().to_path_buf()), maps: vec![1, 2], ..Default::default() });
    wait_for_folder(&mut h);
    h.frames(3);
    // a bookmark on map 1 and one on map 2, both called "gate"
    h.app.switch_to(0);
    let key1 = h.app.bookmark_key().unwrap();
    h.app.switch_to(1);
    let key2 = h.app.bookmark_key().unwrap();
    assert_ne!(key1, key2);
    for k in [&key1, &key2] {
        h.app.settings.bookmarks.insert(k.clone(), vec![crate::settings::Bookmark { name: "gate".into(), x: 1.0, z: 2.0, zoom: 1.0 }]);
    }
    // the prompt opens on map 2; switching to map 1 closes it
    h.app.q3.prompt = Some(q3::TextPrompt::rename_bookmark(key2.clone(), "gate".to_string()));
    h.frames(2);
    assert!(h.app.q3.prompt.is_some());
    h.app.switch_to(0);
    h.frames(2);
    assert!(h.app.q3.prompt.is_none(), "a tab switch closes the rename");
    assert_eq!(h.app.settings.bookmarks[&key1][0].name, "gate");
    assert_eq!(h.app.settings.bookmarks[&key2][0].name, "gate");
    // finishing a rename that belongs to map 2 while map 1 is shown renames map 2's bookmark only
    h.app.finish_prompt(q3::TextPrompt::rename_bookmark_to(key2.clone(), "gate".to_string(), "north gate".to_string()));
    assert_eq!(h.app.settings.bookmarks[&key2][0].name, "north gate");
    assert_eq!(h.app.settings.bookmarks[&key1][0].name, "gate");
}

// ------------------------------------------------------------------------------------------------------------------ heatmap and level range

/// A one-map app (size 64) whose map holds the given monster areas.
fn spawn_app(data: &Path, maps: &Path, areas: Vec<shaiya_svmap::MobArea>) -> Harness {
    let mut h = layout_app(data, maps);
    let d = &mut h.app.docs[0];
    let at = d.map.mob_areas.len();
    let ops = areas.into_iter().enumerate().map(|(i, a)| Op::insert(at + i, Rec::Mob(a))).collect();
    d.commit("test areas", ops);
    h.frames(2);
    h
}

fn area_at(x: f32, z: f32, mob: u32, count: u32) -> shaiya_svmap::MobArea {
    shaiya_svmap::MobArea { min: [x, 0.0, z], max: [x + 8.0, 2.0, z + 8.0], mobs: vec![shaiya_svmap::MobEntry { mob, count }] }
}

#[test]
fn heat_key_cycles_and_the_status_bar_shows_the_tile() {
    let data = TempDir::new("r2-heat-data");
    let maps = TempDir::new("r2-heat-maps");
    let mut h = spawn_app(data.path(), maps.path(), vec![area_at(28.0, 28.0, 3, 6)]);
    assert_eq!(h.app.settings.heat.mode, crate::heat::Mode::Off);
    assert!(h.app.r2.heat_is_none());
    // H: density (the first press), drawn at once
    h.key(egui::Key::H, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.settings.heat.mode, crate::heat::Mode::Density);
    assert!(!h.app.r2.heat_is_none(), "the heatmap picture exists");
    // the cursor over the area: the tile's value in the status bar
    let c = h.app.vp.rect.center();
    h.frame(vec![Event::PointerMoved(c)]);
    h.frames(2);
    assert!(h.text.contains("per tile"), "{}", h.text);
    // without client data level mode is skipped: the next H switches off
    h.key(egui::Key::H, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.settings.heat.mode, crate::heat::Mode::Off);
    assert!(h.app.r2.heat_is_none() && !h.text.contains("per tile"));
    assert!(h.app.can_run(Cmd::HeatLevel).is_err(), "level needs the client data");
    // View > Heatmap > Density by its command
    let ctx = h.ctx.clone();
    h.app.run(Cmd::HeatDensity, &ctx);
    assert_eq!(h.app.settings.heat.mode, crate::heat::Mode::Density);
    // hidden monster areas count nothing
    h.app.settings.layers.mob_areas = false;
    h.frames(2);
    assert_eq!(h.app.r2.heat_sum(), Some(0.0));
    // the options survive a restart
    h.app.settings.layers.mob_areas = true;
    h.app.settings.heat.tile = 16;
    h.app.flush_settings();
    drop(h);
    let h2 = Harness::new(Some(data.path()), StartOptions::default());
    assert_eq!((h2.app.settings.heat.mode, h2.app.settings.heat.tile), (crate::heat::Mode::Density, 16));
}

#[test]
fn an_edit_reaches_the_heatmap_after_the_debounce() {
    let data = TempDir::new("r2-heat-edit-data");
    let maps = TempDir::new("r2-heat-edit-maps");
    let mut h = spawn_app(data.path(), maps.path(), vec![area_at(4.0, 4.0, 3, 6)]);
    h.app.settings.heat.mode = crate::heat::Mode::Density;
    h.frames(2);
    assert!((h.app.r2.heat_sum().unwrap() - 6.0).abs() < 1e-3);
    let d = &mut h.app.docs[0];
    let n = d.map.mob_areas.len();
    d.commit("more", vec![Op::insert(n, Rec::Mob(area_at(40.0, 40.0, 3, 4)))]);
    h.frames(2);
    assert!((h.app.r2.heat_sum().unwrap() - 6.0).abs() < 1e-3, "the old picture stays for a moment");
    // the edit has settled: the debounce is over now (no waiting for the clock)
    h.app.r2.debounce_due();
    h.frames(2);
    assert!((h.app.r2.heat_sum().unwrap() - 10.0).abs() < 1e-3, "{:?}", h.app.r2.heat_sum());
}

#[test]
fn the_spawns_tab_opens_on_demand_and_lists_findings() {
    let data = TempDir::new("r2-spawns-data");
    let maps = TempDir::new("r2-spawns-maps");
    let dup = area_at(10.0, 10.0, 7, 2);
    let mut h = spawn_app(data.path(), maps.path(), vec![dup.clone(), dup, area_at(30.0, 30.0, 8, 3)]);
    assert!(!h.app.settings.layout.dock.visible);
    let ctx = h.ctx.clone();
    assert!(h.app.can_run(Cmd::SpawnCheck).is_ok());
    h.app.run(Cmd::SpawnCheck, &ctx);
    h.frames(3);
    assert!(h.app.dock_showing(DockTab::Spawns), "{:?}", h.app.settings.layout);
    for w in ["this map", "all maps", "tolerance", "Level range", "Duplicate areas"] {
        assert!(h.text.contains(w), "{w}: {}", h.text);
    }
    // no client: levels are not checked and the table lists the monsters by id
    assert!(h.text.contains("monster 7") && h.text.contains("monster 8"), "{}", h.text);
    let report = h.app.r2.data.as_ref().unwrap().report.clone();
    assert_eq!(report.findings.iter().filter(|f| f.group == crate::spawns::Group::Duplicate).count(), 1);
    // a finding row selects both areas of the pair
    h.app.open_spawn_finding(report.findings.iter().position(|f| f.group == crate::spawns::Group::Duplicate).unwrap());
    assert_eq!(h.app.docs[0].selection, vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 1)]);
    // F1 with the Spawns tab open and nothing selected opens the guide at its page
    h.app.docs[0].selection.clear();
    h.app.open_help_for_state();
    assert!(h.app.help_open && h.app.help.scroll_to.as_deref() == Some("the-spawns-tab"));
    h.app.help_open = false;
    // the all-maps scope shows the folder line
    h.app.r2.ui.all_maps = true;
    h.frames(3);
    assert!(h.text.contains("maps") && h.text.contains("kinds"), "{}", h.text);
    // an edit that removes the duplicate updates the tab
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 1)];
    h.app.docs[0].delete_selected();
    h.frames(2);
    assert_eq!(h.app.r2.data.as_ref().unwrap().report.findings.len(), 0);
    assert_eq!(h.app.spawn_tab_count(), Some(0));
}

/// The folder-scope totals of the app and the CLI come from the same code on the same maps.
#[test]
fn spawns_cli_matches_panel() {
    let (Some(maps), Some(client)) = (std::env::var_os("SVMAP_ORIGINALS"), std::env::var_os("SHAIYA_CLIENT")) else {
        eprintln!("skipped: SVMAP_ORIGINALS and SHAIYA_CLIENT are not both set");
        return;
    };
    let mut h = Harness::new(None, StartOptions { folder: Some(PathBuf::from(&maps)), client: Some(PathBuf::from(&client)), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    assert!(h.app.have_levels(), "the client's monsters are loaded");
    let (totals, never) = h.app.spawn_totals().expect("spawn data");
    let d = shaiya_svmap_tools::batch::spawns(Path::new(&maps), Some(Path::new(&client)), &shaiya_core::Job::silent()).unwrap();
    let json = serde_json::to_value(&d).unwrap();
    let t = &json["totals"];
    assert_eq!(t["maps"].as_u64(), Some(totals.maps as u64));
    assert_eq!(t["areas"].as_u64(), Some(totals.areas as u64));
    assert_eq!(t["monsters"].as_u64(), Some(totals.monsters));
    assert_eq!(t["kinds"].as_u64(), Some(totals.kinds as u64));
    for g in crate::spawns::Group::ALL {
        assert_eq!(t["findings"][g.key()].as_u64(), Some(totals.findings[g.index()] as u64), "{}", g.key());
    }
    assert_eq!(t["never_spawned"].as_u64(), never.map(|n| n as u64));
    assert_eq!(totals.maps, 77);
    println!("spawns totals: {} maps, {} areas, {} monsters, {} kinds, findings {:?}, never spawned {:?}", totals.maps, totals.areas, totals.monsters, totals.kinds, totals.findings, never);
}

/// Monster.SData records with a name and a level that no map of the 77 uses, counted without the spawns code.
#[test]
fn never_spawned_originals() {
    let (Some(maps), Some(client)) = (std::env::var_os("SVMAP_ORIGINALS"), std::env::var_os("SHAIYA_CLIENT")) else {
        eprintln!("skipped: SVMAP_ORIGINALS and SHAIYA_CLIENT are not both set");
        return;
    };
    let rules = ClientRules::open(Path::new(&client)).unwrap();
    let monsters = rules.monsters.as_ref().expect("Monster.SData");
    let mut used: HashSet<u32> = HashSet::new();
    let mut files = 0;
    for e in std::fs::read_dir(&maps).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "svmap") {
            let m = shaiya_svmap::read(&std::fs::read(e.path()).unwrap()).unwrap();
            used.extend(m.mob_areas.iter().flat_map(|a| a.mobs.iter().map(|x| x.mob)));
            files += 1;
        }
    }
    assert_eq!(files, 77);
    let named: Vec<usize> = (0..monsters.records.len()).filter(|i| !monsters.records[*i].name.trim().is_empty() && monsters.records[*i].level > 0).collect();
    let want = named.iter().filter(|i| !used.contains(&(**i as u32))).count();
    let d = shaiya_svmap_tools::batch::spawns(Path::new(&maps), Some(Path::new(&client)), &shaiya_core::Job::silent()).unwrap();
    assert_eq!(d.never_spawned.as_ref().map(|n| n.len()), Some(want));
    println!("never spawned: {want} of {} named monsters with a level ({} distinct ids used)", named.len(), used.len());
}

/// The level range numbers belong to the map they were opened on; showing another map closes them, so Set cannot
/// store them under the wrong map.
#[test]
fn r2_review_band_edit_follows_its_map() {
    let dir = TempDir::new("r2-band-edit");
    let data = TempDir::new("r2-band-edit-data");
    svmap_file(dir.path(), 1, 64);
    svmap_file(dir.path(), 2, 64);
    let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(dir.path().to_path_buf()), maps: vec![1, 2], ..Default::default() });
    wait_for_folder(&mut h);
    let ctx = h.ctx.clone();
    h.app.run(Cmd::SpawnCheck, &ctx);
    h.app.switch_to(0);
    h.frames(3);
    assert_eq!(h.app.docs[h.app.active].map_id, Some(1));
    assert!(h.text.contains("Level range") && h.text.contains("Edit"), "{}", h.text);
    h.app.r2.ui.band_edit = Some((1, 10, 20));
    h.frames(2);
    assert_eq!(h.app.r2.ui.band_edit, Some((1, 10, 20)), "stays open on its own map");
    assert!(h.text.contains("Set"), "{}", h.text);
    h.app.switch_to(1);
    h.frames(2);
    assert_eq!(h.app.docs[h.app.active].map_id, Some(2));
    assert_eq!(h.app.r2.ui.band_edit, None, "another map closes it");
    assert!(h.app.settings.spawn_bands.is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------- spawn editing

impl Harness {
    /// The screen position of a world point of the active map in the 2D viewport.
    fn world_to_screen(&self, x: f32, z: f32) -> Pos2 {
        let d = &self.app.docs[self.app.active];
        view2d::Xf::new(self.app.vp.rect, d.view.cx, d.view.cz, d.view.zoom).to_screen(x, z)
    }

    /// A drag with the primary button from `a` to `b` in a few frames.
    fn drag(&mut self, a: Pos2, b: Pos2) {
        let down = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![Event::PointerMoved(a)]);
        self.frame(vec![down(a, true)]);
        self.frame(vec![Event::PointerMoved(a + (b - a) * 0.5)]);
        self.frame(vec![Event::PointerMoved(b)]);
        self.frame(vec![down(b, false)]);
        self.frame(vec![]);
    }
}

/// A 512 x 512 map with nothing on it, open in a tab with the Scatter tool active and two monsters listed.
fn scatter_app(maps: &Path) -> Harness {
    let f = svmap_file(maps, 1, 512);
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.frames(4);
    h.app.tool = Tool::Scatter;
    h.app.settings.scatter.mobs = vec![[5, 2], [6, 1]];
    h.frames(2);
    h
}

#[test]
fn scatter_apply_one_step() {
    let maps = TempDir::new("scatter-apply");
    let mut h = scatter_app(maps.path());
    let before = shaiya_svmap::write(&h.app.docs[0].map);
    let (a, b) = (h.world_to_screen(100.0, 100.0), h.world_to_screen(300.0, 300.0));
    h.drag(a, b);
    let region = h.app.vp.scatter.region.clone().expect("the drag made a region");
    let bounds = region.bounds().unwrap();
    assert!((bounds[2] - bounds[0] - 200.0).abs() < 4.0 && (bounds[3] - bounds[1] - 200.0).abs() < 4.0, "{bounds:?}");
    h.frames(2);
    let n = h.app.r3.scatter.cache.as_ref().expect("a preview").preview.areas.len();
    assert!((50..=115).contains(&n), "Normal on about 200 x 200 gives {n} areas");
    assert!(h.text.contains(&format!("{n} areas")) && h.text.contains("monsters"), "the preview line is on the map: {}", h.text);
    assert!(h.app.docs[0].undo.is_empty(), "a preview edits nothing");
    // Enter applies: one step, N new areas at the end, selected, the region gone
    h.key(egui::Key::Enter, Modifiers::NONE);
    let d = &h.app.docs[0];
    assert_eq!(d.map.mob_areas.len(), n);
    assert_eq!(d.undo.len(), 1, "one undo step");
    assert_eq!(d.selection.len(), n);
    for a in &d.map.mob_areas {
        assert!((a.max[1] - a.min[1] - 2.0).abs() < 1e-4, "y2 - y1 = 2");
        assert!(a.mobs.len() == 1 && (a.mobs[0].mob == 5 || a.mobs[0].mob == 6) && a.mobs[0].count == 2);
        assert!(a.min[0] >= 0.0 && a.max[0] <= 512.0);
    }
    assert!(h.app.vp.scatter.region.is_none() && h.app.r3.scatter.cache.is_none());
    // undo restores the bytes
    h.app.docs[0].undo();
    assert_eq!(shaiya_svmap::write(&h.app.docs[0].map), before);
    assert!(!h.app.docs[0].is_dirty());
}

#[test]
fn scatter_polygon_keys_and_the_heatmap() {
    let maps = TempDir::new("scatter-poly");
    let mut h = scatter_app(maps.path());
    for (x, z) in [(100.0, 100.0), (300.0, 100.0), (300.0, 300.0), (100.0, 300.0)] {
        let p = h.world_to_screen(x, z);
        h.click(p);
    }
    assert_eq!(h.app.vp.scatter.poly.len(), 4);
    assert!(h.app.vp.scatter.region.is_none(), "still a polygon in the making");
    // Backspace takes the last point back; clicking the first point closes it
    h.key(egui::Key::Backspace, Modifiers::NONE);
    assert_eq!(h.app.vp.scatter.poly.len(), 3);
    let first = h.world_to_screen(100.0, 100.0);
    h.click(first);
    let region = h.app.vp.scatter.region.clone().expect("a click on the first point closes it");
    assert!(matches!(region.shapes[0], scatter::Shape::Poly(ref p) if p.len() == 3), "{region:?}");
    h.frames(2);
    let pv = h.app.r3.scatter.cache.as_ref().unwrap().preview.clone();
    assert!(!pv.areas.is_empty());
    for a in &pv.areas {
        assert!(region.contains((a.min[0] + a.max[0]) * 0.5, (a.min[2] + a.max[2]) * 0.5), "inside the triangle");
    }
    // the heatmap counts the preview
    h.app.settings.heat.mode = crate::heat::Mode::Density;
    h.frames(3);
    let sum = h.app.r2.heat_sum().expect("heatmap on");
    let want: f32 = pv.areas.iter().map(|a| a.mobs.iter().map(|e| e.count as f32).sum::<f32>()).sum();
    assert!((sum - want).abs() < 0.5, "heat {sum} vs preview {want}");
    // R reseeds: another seed, another preview
    let seed = h.app.settings.scatter.seed;
    h.key(egui::Key::R, Modifiers::NONE);
    assert_ne!(h.app.settings.scatter.seed, seed);
    h.frames(2);
    assert_ne!(h.app.r3.scatter.cache.as_ref().unwrap().preview.areas, pv.areas);
    // Esc cancels the region, the second Esc leaves the tool; the heat has no preview any more
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(h.app.vp.scatter.region.is_none() && h.app.tool == Tool::Scatter);
    h.frames(3);
    assert!(h.app.r3.scatter.cache.is_none());
    assert!(h.app.r2.heat_sum().unwrap() < 0.5, "an empty map has no heat");
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Select);
    // Enter with no monsters says so and adds nothing
    h.app.tool = Tool::Scatter;
    h.app.settings.scatter.mobs.clear();
    h.app.vp.scatter.set(Region::rect([100.0, 100.0], [200.0, 200.0]));
    h.frames(2);
    assert!(h.text.contains("Add a monster first."), "{}", h.text);
    h.key(egui::Key::Enter, Modifiers::NONE);
    assert!(h.app.docs[0].map.mob_areas.is_empty() && h.app.docs[0].undo.is_empty());
}

#[test]
fn scatter_fill_selection_and_from_template() {
    let maps = TempDir::new("scatter-fill");
    let mut h = scatter_app(maps.path());
    // nothing selected: a toast and no region
    h.app.scatter_fill_selection();
    assert!(h.app.vp.scatter.region.is_none());
    // two selected monster areas are the region (the union of their boxes)
    let areas = [mob_area(10.0, &[(1, 1)]), mob_area(100.0, &[(1, 1)])];
    for (i, r) in areas.into_iter().enumerate() {
        h.app.docs[0].commit("add", vec![Op::insert(i, r)]);
    }
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 1)];
    h.app.scatter_fill_selection();
    let r = h.app.vp.scatter.region.clone().unwrap();
    assert_eq!(r.shapes.len(), 2);
    assert!(r.contains(11.0, 11.0) && r.contains(101.0, 12.0) && !r.contains(50.0, 12.0));
    // a template's monsters become the list, counts as weights (clamped to 1..9)
    h.app.settings.templates = vec![crate::settings::Template { name: "wolves".into(), entries: vec![[5, 3], [6, 20], [5, 1]], ..Default::default() }];
    h.app.scatter_from_template(0);
    assert_eq!(h.app.settings.scatter.mobs, vec![[5, 3], [6, 9]]);
}

#[test]
fn bulk_plan_and_apply() {
    let maps = TempDir::new("bulk-apply");
    let disk = three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    // map 1's third area (monster 5) is already at 8 (unsaved)
    let before = match edit::get(&h.app.docs[0].map, SectionId::MobAreas, 2) {
        Some(Rec::Mob(a)) => a,
        _ => unreachable!(),
    };
    let mut at8 = before.clone();
    at8.mobs[0].count = 8;
    h.app.docs[0].commit("count 8", vec![Op::replace(2, Rec::Mob(before), Rec::Mob(at8))]);
    let rule = crate::bulk::Rule::parse("map:1-2 mob:5 => count +1").unwrap();
    let s = h.app.bulk_apply(&rule, crate::bulk::Opts::default(), None);
    assert!(s.contains("in 3 areas on 2 maps") && s.contains("Held at 8: 1") && s.contains("nothing is saved"), "{s}");
    // map 1: the edit and the bulk step; map 2 (opened for it): one step; map 3 is not in the rule and not opened
    let d1 = h.app.docs.iter().find(|d| d.map_id == Some(1)).unwrap();
    let d2 = h.app.docs.iter().find(|d| d.map_id == Some(2)).unwrap();
    assert!(h.app.docs.iter().all(|d| d.map_id != Some(3)));
    assert_eq!((d1.undo.len(), d2.undo.len()), (2, 1), "one step per map");
    let counts = |d: &Doc, i: usize| d.map.mob_areas[i].mobs.iter().map(|e| (e.mob, e.count)).collect::<Vec<_>>();
    for d in [d1, d2] {
        assert_eq!(counts(d, 0), vec![(5, 3), (7, 1)], "only monster 5 changes");
        assert_eq!(counts(d, 1), vec![(7, 3)]);
    }
    assert_eq!(counts(d1, 2), vec![(5, 8)], "held at 8");
    assert_eq!(counts(d2, 2), vec![(5, 2)]);
    assert!(d1.is_dirty() && d2.is_dirty());
    // nothing was saved
    for (id, bytes) in &disk {
        assert_eq!(&std::fs::read(maps.join(&format!("{id}.svmap"))).unwrap(), bytes, "map {id} on disk");
    }
    // the step is labelled with the rule; undo per map restores
    assert_eq!(d1.undo.last().map(|s| s.label.as_str()), Some("bulk: map:1-2 mob:5 => count +1"));
    for d in h.app.docs.iter_mut() {
        d.undo();
    }
    let d1 = h.app.docs.iter().find(|d| d.map_id == Some(1)).unwrap();
    let d2 = h.app.docs.iter().find(|d| d.map_id == Some(2)).unwrap();
    assert_eq!((counts(d1, 0), counts(d1, 2)), (vec![(5, 2), (7, 1)], vec![(5, 8)]));
    assert!(!d2.is_dirty() && counts(d2, 0) == vec![(5, 2), (7, 1)]);
    // allow above 8: the area at 8 grows
    let rule = crate::bulk::Rule::parse("map:1 mob:5 => count +1").unwrap();
    h.app.bulk_apply(&rule, crate::bulk::Opts { allow_above_8: true }, None);
    assert_eq!(counts(h.app.docs.iter().find(|d| d.map_id == Some(1)).unwrap(), 2), vec![(5, 9)]);
}

#[test]
fn bulk_skips_read_only() {
    let maps = TempDir::new("bulk-ro");
    let disk = three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_read_only(&maps.join("2.svmap"), None, Some(2));
    h.frames(2);
    let ro = h.app.docs[0].map.clone();
    let rule = crate::bulk::Rule::parse("mob:5 => delete").unwrap();
    let s = h.app.bulk_apply(&rule, crate::bulk::Opts::default(), None);
    assert!(s.contains("Skipped: 2 (read-only tab)"), "{s}");
    assert!(s.contains("in 4 areas on 2 maps"), "{s}");
    let d = h.app.docs.iter().find(|d| d.read_only).unwrap();
    assert_eq!(d.map, ro);
    assert!(d.undo.is_empty() && !d.is_dirty());
    for e in h.app.docs.iter().filter(|d| !d.read_only) {
        assert_eq!(e.undo.len(), 1);
        assert_eq!(e.map.mob_areas.len(), 1, "both areas with monster 5 are gone");
    }
    for (id, bytes) in &disk {
        assert_eq!(&std::fs::read(maps.join(&format!("{id}.svmap"))).unwrap(), bytes);
    }
}

/// Every spawn edit path leaves a read-only tab alone: Scatter apply and Bulk apply.
#[test]
fn read_only_refuses_r_edits() {
    let maps = TempDir::new("r-ro");
    let f = svmap_file(maps.path(), 7, 512);
    let mut h = Harness::new(None, StartOptions::default());
    h.app.open_read_only(&f, None, Some(7));
    h.frames(3);
    let before = h.app.docs[0].map.clone();
    // Scatter: the tool is greyed, Apply is refused even when the region and the monsters are there
    h.app.settings.scatter.mobs = vec![[5, 2]];
    h.app.tool = Tool::Scatter;
    h.app.vp.scatter.set(Region::rect([100.0, 100.0], [300.0, 300.0]));
    h.frames(2);
    assert_eq!(h.app.effective_tool(), Tool::Select, "the tool is not usable on this tab");
    assert!(h.app.tool_blocked(Tool::Scatter).is_some());
    h.app.scatter_apply();
    h.key(egui::Key::Enter, Modifiers::NONE);
    assert_eq!(h.app.docs[0].map, before);
    assert!(h.app.docs[0].undo.is_empty() && !h.app.docs[0].is_dirty());
    // the document itself refuses the same ops
    let ops: Vec<Op> = (0..3).map(|i| Op::insert(i, mob_area(i as f32 * 10.0, &[(5, 1)]))).collect();
    h.app.docs[0].commit("scatter 3 areas", ops);
    assert_eq!(h.app.docs[0].map, before);
    assert!(h.app.docs[0].refused > 0);
    // Bulk: a map with only a read-only tab is skipped and named
    let mut m = Svmap::new(64);
    m.mob_areas.push(match mob_area(0.0, &[(5, 2)]) {
        Rec::Mob(a) => a,
        _ => unreachable!(),
    });
    let f2 = maps.join("8.svmap");
    std::fs::write(&f2, shaiya_svmap::write(&m)).unwrap();
    h.app.open_read_only(&f2, None, Some(8));
    h.frames(2);
    let ro8 = h.app.docs.iter().position(|d| d.map_id == Some(8)).unwrap();
    let rule = crate::bulk::Rule::parse("mob:5 => count +1").unwrap();
    let s = h.app.bulk_apply(&rule, crate::bulk::Opts::default(), None);
    assert!(s.contains("Skipped: 8 (read-only tab)") && s.contains("in 0 areas on 0 maps"), "{s}");
    assert_eq!(h.app.docs[ro8].map, m);
    assert!(h.app.docs[ro8].undo.is_empty());
    assert_eq!(h.app.docs.len(), 2, "no editable tab was opened for it");
    // Take, Take section, Take cells and Take whole map against another folder
    let other = TempDir::new("r-ro-other");
    let mut o = m.clone();
    o.mob_areas[0].min[0] += 4.0;
    o.mob_areas[0].max[0] += 4.0;
    o.mob_areas.push(o.mob_areas[0].clone());
    o.blocked.set(2, 2, true);
    std::fs::write(other.join("8.svmap"), shaiya_svmap::write(&o)).unwrap();
    h.app.settings.other_folder = Some(other.path().display().to_string());
    h.app.settings.compare_against = CompareAgainst::Other;
    h.app.settings.layers.compare = true;
    h.app.open_dock(DockTab::Changes);
    h.app.active = ro8;
    h.frames(3);
    let uid = h.app.docs[ro8].uid;
    let rows = h.app.safety.cmp.get(&uid).map(|c| c.rows.clone()).unwrap_or_default();
    assert!(!rows.is_empty(), "the other folder's copy differs");
    for a in [safety::CAct::Revert(rows[0]), safety::CAct::Section(SectionId::MobAreas), safety::CAct::Cells, safety::CAct::Whole] {
        h.app.compare_act(uid, true, a);
    }
    h.frames(2);
    assert_eq!(h.app.docs[ro8].map, m);
    assert!(h.app.docs[ro8].undo.is_empty() && h.app.dialog.is_none());
}

#[test]
fn the_bulk_window_shows_the_plan_and_saves_rules() {
    let maps = TempDir::new("bulk-window");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    let ctx = h.ctx.clone();
    assert!(h.app.can_run(Cmd::BulkEdit).is_ok());
    h.app.run(Cmd::BulkEdit, &ctx);
    h.frames(2);
    assert!(h.text.contains("Bulk edit spawns") && h.text.contains("Type a rule"), "{}", h.text);
    // a rule: the plan is worked out for all three maps (monster 5 is in two areas of each)
    h.app.r3.bulk.as_mut().unwrap().text = "mob:5 => count +1".to_string();
    h.frames(3);
    assert!(h.text.contains("6 areas match on 3 maps. 6 to change on 3 maps."), "{}", h.text);
    assert!(h.text.contains("Apply to 6 areas on 3 maps"), "{}", h.text);
    // the active map's matches are outlined
    assert_eq!(h.app.bulk_outline(), vec![0, 2]);
    // a typo is named and nothing can be applied
    h.app.r3.bulk.as_mut().unwrap().text = "mob:x => count +1".to_string();
    h.frames(3);
    assert!(h.text.contains("the value does not fit mob:"), "{}", h.text);
    assert!(h.app.bulk_outline().is_empty());
    // saved rules show in the window
    assert!(h.app.settings.bulk_rules.is_empty());
    h.app.settings.bulk_rules.push(crate::settings::BulkRule { name: "more wolves".into(), text: "mob:5 => count +1".into() });
    h.frames(2);
    assert!(h.text.contains("Saved rules (1)") && h.text.contains("more wolves"), "{}", h.text);
    // Find's Replace monster... opens the window with the action filled in
    h.app.q3.find.query = "mob:5".to_string();
    h.app.open_bulk_from_find(Some(5));
    assert_eq!(h.app.r3.bulk.as_ref().unwrap().text, "mob:5 => replace 5 with 5");
    h.app.q3.find.this_map = true;
    h.app.open_bulk_from_find(None);
    assert_eq!(h.app.r3.bulk.as_ref().unwrap().text, "map:1 mob:5 => count +1");
}

/// Find's Replace monster... opens Bulk edit with "replace 5 with 5", which is not a valid rule yet. The action builder
/// still shows a replace (not "count +"), and picking the second monster fills the right side.
#[test]
fn r3_review_replace_from_find_picks_the_new_monster() {
    let maps = TempDir::new("r3-review-replace");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.q3.find.query = "mob:5".to_string();
    h.app.open_bulk_from_find(Some(5));
    h.frames(3);
    assert!(h.text.contains("pick two different monsters"), "{}", h.text);
    assert!(h.text.contains("Pick"), "the builder shows the replace with its Pick buttons: {}", h.text);
    h.app.apply_pick(r3_ui::PickTarget::BulkTo, 9);
    assert_eq!(h.app.r3.bulk.as_ref().unwrap().text, "mob:5 => replace 5 with 9");
    h.app.apply_pick(r3_ui::PickTarget::BulkFrom, 7);
    assert_eq!(h.app.r3.bulk.as_ref().unwrap().text, "mob:5 => replace 7 with 9");
    h.app.r3.bulk.as_mut().unwrap().text = "mob:5 => replace 5 with 9".to_string();
    h.frames(3);
    assert!(h.text.contains("6 areas match on 3 maps. 6 to change on 3 maps."), "{}", h.text);
}

#[test]
fn ctrl_c_with_measure_copies_the_measurement_and_esc_clears_it_first() {
    let maps = TempDir::new("measure-copy");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_map_id(1);
    h.frames(2);
    h.app.tool = Tool::Measure;
    h.frames(2);
    h.app.vp.measure.click([10.0, 10.0]);
    h.app.vp.measure.click([40.0, 50.0]);
    h.app.vp.measure.finish();
    h.frame(vec![Event::Copy]);
    assert_eq!(h.copied, vec!["length 50.0 (1 segment)".to_string()]);
    // a selection does not change what Copy takes while a measurement is there
    h.app.docs[0].select_only(SectionId::MobAreas, 0);
    h.frame(vec![Event::Copy]);
    assert_eq!(h.copied, vec!["length 50.0 (1 segment)".to_string()]);
    // Esc clears the measurement and keeps the tool; the second Esc leaves the tool
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert!(!h.app.vp.measure.active());
    assert_eq!(h.app.tool, Tool::Measure);
    h.key(egui::Key::Escape, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Select);
    // with nothing measured Copy takes the records again
    h.frame(vec![Event::Copy]);
    assert!(h.copied.len() == 1 && h.copied[0].starts_with("{\"svmap-editor\":1"), "{:?}", h.copied);
    // the Radius option is in the toolbar while the tool is active
    h.app.tool = Tool::Measure;
    h.frames(2);
    assert!(h.text.contains("Radius"), "{}", h.text);
}

#[test]
fn the_3d_view_keeps_measure_and_box_usable() {
    let maps = TempDir::new("measure-3d");
    three_maps(maps.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.path().to_path_buf()), maps: vec![1], ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_map_id(1);
    h.frames(2);
    h.app.mode3d = true;
    assert_eq!(h.app.tool_blocked(Tool::Measure), None, "Measure works in 3D");
    assert_eq!(h.app.tool_blocked(Tool::Box), None);
    assert_eq!(h.app.tool_blocked(Tool::Paint), Some("2D only"));
    assert_eq!(h.app.tool_blocked(Tool::Scatter), Some("2D only"));
    // the measurement of the 3D view is its own: Ctrl+C and Esc reach it
    let uid = h.app.docs[0].uid;
    h.app.tool = Tool::Measure;
    let st = h.app.v3.states.entry(uid).or_default();
    st.measure.click([5.0, 5.0]);
    st.measure.click([5.0, 35.0]);
    st.measure.finish();
    assert_eq!(h.app.measure_text().as_deref(), Some("length 30.0 (1 segment)"));
    assert!(h.app.clear_measure());
    assert!(h.app.measure_text().is_none());
    assert!(!h.app.clear_measure());
}

#[test]
fn read_only_refuses_the_3d_and_context_menu_edits() {
    let maps = TempDir::new("r4-ro");
    let mut m0 = Svmap::new(256);
    m0.mob_areas.push(shaiya_svmap::MobArea { min: [20.0, 0.0, 20.0], max: [30.0, 2.0, 30.0], mobs: vec![shaiya_svmap::MobEntry { mob: 5, count: 1 }] });
    let f = maps.join("7.svmap");
    std::fs::write(&f, shaiya_svmap::write(&m0)).unwrap();
    let mut h = Harness::new(None, StartOptions::default());
    h.app.open_read_only(&f, None, Some(7));
    h.frames(3);
    let before = h.app.docs[0].map.clone();
    // the context menu's edits
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 0)];
    let ctx = h.ctx.clone();
    for act in [view2d::CtxAct::AddArea([10.0, 10.0]), view2d::CtxAct::AddNpc([10.0, 10.0]), view2d::CtxAct::PasteHere([10.0, 10.0]), view2d::CtxAct::Delete, view2d::CtxAct::Duplicate, view2d::CtxAct::DropToGround] {
        h.app.do_ctx_act(act, &ctx);
    }
    assert_eq!(h.app.docs[0].map, before);
    assert!(h.app.docs[0].undo.is_empty() && !h.app.docs[0].is_dirty());
    // the 3D Box tool and the template stamp
    let flat = |_: f32, _: f32| Some(0.0);
    let g = crate::view3d::Ground3 { size: 256.0, h: &flat };
    let cam = crate::view3d::Camera3d { focus: [128.0, 0.0, 128.0], pitch: -80f32.to_radians(), dist: 150.0, ..Default::default() };
    let r0 = cam.ray([-0.2, -0.2], 1.5);
    let r1 = cam.ray([0.2, 0.2], 1.5);
    let start = crate::view3d::snapped_ground(&g, &r0, 1.0).unwrap();
    assert_eq!(crate::view3d::end_box_draw(&mut h.app.docs[0], start, Some(&r1), None, &g, 1.0, view2d::DrawTarget::MobArea, None), None);
    let tpl = crate::settings::Template { name: "t".into(), size: [8.0, 8.0], height: 2.0, entries: vec![[5, 1]] };
    view2d::stamp_template(&mut h.app.docs[0], &|_, _| 0.0, [50.0, 50.0], 1.0, &tpl);
    assert_eq!(h.app.docs[0].map, before);
    assert!(h.app.docs[0].undo.is_empty());
    // the same calls on an editable map do edit it (the test is not vacuous)
    let mut m = Svmap::new(256);
    m.mob_areas.push(shaiya_svmap::MobArea { min: [0.0, 0.0, 0.0], max: [4.0, 2.0, 4.0], mobs: vec![] });
    let mut d = Doc::new(1, Some(7), None, m, None);
    crate::view3d::end_box_draw(&mut d, start, Some(&r1), None, &g, 1.0, view2d::DrawTarget::MobArea, None);
    assert_eq!(d.map.mob_areas.len(), 2);
}

// ---------------------------------------------------------------------------------------------------------------------- world and folders

fn mob_of(m: &Svmap, i: usize) -> shaiya_svmap::MobArea {
    m.mob_areas[i].clone()
}

fn read_map(p: &Path) -> Svmap {
    shaiya_svmap::read(&std::fs::read(p).unwrap()).unwrap()
}

fn portal_to(to: u32) -> shaiya_svmap::Portal {
    shaiya_svmap::Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 0, level_max: 0, to_map: to, to_pos: [9.0, 0.0, 9.0] }
}

/// Folder A: three maps (areas only). Folder B: map 1 has an area moved, map 2 lost an area, map 3 gained a portal, map 9 is new.
fn folders_ab(tag: &str) -> (TempDir, TempDir) {
    let (a, b) = (TempDir::new(&format!("{tag}-a")), TempDir::new(&format!("{tag}-b")));
    three_maps(a.path());
    three_maps(b.path());
    let edit = |id: u32, f: &dyn Fn(&mut Svmap)| {
        let p = b.join(&format!("{id}.svmap"));
        let mut m = read_map(&p);
        f(&mut m);
        std::fs::write(&p, shaiya_svmap::write(&m)).unwrap();
    };
    edit(1, &|m| {
        m.mob_areas[0].min[0] += 5.0;
        m.mob_areas[0].max[0] += 5.0;
        m.blocked.set(3, 3, true);
    });
    edit(2, &|m| {
        m.mob_areas.remove(1);
    });
    edit(3, &|m| m.portals.push(portal_to(1)));
    let mut nine = Svmap::new(64);
    nine.mob_areas.push(mob_of(&read_map(&b.join("1.svmap")), 0));
    std::fs::write(b.join("9.svmap"), shaiya_svmap::write(&nine)).unwrap();
    (a, b)
}

fn scan_done(h: &mut Harness) {
    wait_until(h, "the folder comparison", WAIT, |h| h.app.r5.fd.scanned.is_some() && !h.app.r5.fd.running());
    h.frames(2);
}

/// Compare folders lists every id, Take brings one record, one section, the cells or the whole map over (one undo step each), and
/// the map ends equal to the other folder's file.
#[test]
fn folder_diff_take() {
    let (a, b) = folders_ab("fdt");
    let snapshot = |dir: &Path| -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
        v.sort();
        v
    };
    let other_before = snapshot(b.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(a.path().to_path_buf()), ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_compare_folders();
    h.app.r5.fd.other = b.path().display().to_string();
    scan_done(&mut h);
    let st = |h: &Harness, id: u32| h.app.r5.fd.rows.iter().find(|r| r.id == id).map(|r| r.status.label());
    assert_eq!((st(&h, 1), st(&h, 2), st(&h, 3), st(&h, 9)), (Some("differs"), Some("differs"), Some("differs"), Some("only there")));
    assert!(h.text.contains("only there") && h.text.contains("Copy into this folder") && h.text.contains("Open pair"), "{}", h.text);
    let t = folderdiff::totals(&h.app.r5.fd.rows);
    assert_eq!((t.same, t.differs, t.only_there), (0, 3, 1));

    // open the pair (a double-click on the row's status): the tab shows the map, the Changes tab compares against the other folder
    let p = h.text_pos("differs").expect("a differing row");
    let click = |pressed| Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.frame(vec![Event::PointerMoved(p)]);
    h.frame(vec![click(true)]);
    h.frame(vec![click(false)]);
    h.frame(vec![click(true)]);
    h.frame(vec![click(false)]);
    h.frames(3);
    assert_eq!(h.app.r5.showing, Showing::Map);
    assert_eq!(h.app.docs[h.app.active].map_id, Some(1));
    assert_eq!(h.app.settings.compare_against, CompareAgainst::Other);
    assert!(h.app.dock_showing(DockTab::Changes) && h.app.settings.layers.compare);
    assert!(h.text.contains("other folder") && h.text.contains("Take whole map") && h.text.contains("Take cells") && h.text.contains("Take section"), "{}", h.text);
    let uid = h.app.docs[h.app.active].uid;
    let before = h.app.docs[h.app.active].map.clone();
    let other = read_map(&b.join("1.svmap"));
    let row = h.app.safety.cmp.get(&uid).unwrap().rows[0];

    // Take one record
    h.app.compare_act(uid, true, safety::CAct::Revert(row));
    h.frames(2);
    {
        let d = &h.app.docs[h.app.active];
        assert_eq!(d.map.mob_areas[0], other.mob_areas[0]);
        assert_eq!(d.undo.len(), 1);
        assert!(d.undo[0].label.starts_with("take"), "{}", d.undo[0].label);
    }
    h.app.docs[h.app.active].undo();
    assert_eq!(h.app.docs[h.app.active].map, before);
    // Take cells
    h.frames(2);
    h.app.compare_act(uid, true, safety::CAct::Cells);
    assert_eq!(h.app.docs[h.app.active].map.blocked, other.blocked);
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1);
    h.app.docs[h.app.active].undo();
    // Take section
    h.frames(2);
    h.app.compare_act(uid, true, safety::CAct::Section(SectionId::MobAreas));
    assert_eq!(h.app.docs[h.app.active].map.mob_areas, other.mob_areas);
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1);
    h.app.docs[h.app.active].undo();
    // Take the whole map: one step, and the saved file is the other file
    h.frames(2);
    h.app.compare_act(uid, true, safety::CAct::Whole);
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1);
    assert_eq!(shaiya_svmap::write(&h.app.docs[h.app.active].map), std::fs::read(b.join("1.svmap")).unwrap());
    let out = TempDir::new("fdt-out");
    h.app.docs[h.app.active].save(Some(&out.join("1.svmap"))).unwrap();
    assert_eq!(std::fs::read(out.join("1.svmap")).unwrap(), std::fs::read(b.join("1.svmap")).unwrap());
    h.app.docs[h.app.active].undo();
    assert_eq!(h.app.docs[h.app.active].map, before, "undo restores the map");
    assert_eq!(std::fs::read(a.join("1.svmap")).unwrap(), shaiya_svmap::write(&before), "nothing was written into this folder");

    // map 3: the other copy has one portal more; taking it asks first (the client mirrors the portal count)
    h.app.open_pair(3);
    h.frames(3);
    let uid3 = h.app.docs[h.app.active].uid;
    h.app.compare_act(uid3, true, safety::CAct::Whole);
    match &h.app.dialog {
        Some(Dialog::Confirm { title, text, .. }) => {
            assert!(title.starts_with("Take changes portals"), "{title}");
            assert!(text.contains("This take adds 1"), "{text}");
        }
        _ => panic!("the portal count change should ask first"),
    }
    assert!(h.app.docs[h.app.active].undo.is_empty());
    if let Some(Dialog::Confirm { action, .. }) = h.app.dialog.take() {
        h.app.apply(action);
    }
    assert_eq!(h.app.docs[h.app.active].map.portals.len(), 1);
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1);
    // saving the taken map writes this folder only: the other folder is byte for byte as it was, with no .bak or .tmp
    let uid3 = h.app.docs[h.app.active].uid;
    h.app.begin_save_ext(uid3, None, true, false);
    assert!(!h.app.docs[h.app.active].is_dirty());
    assert_eq!(std::fs::read(a.join("3.svmap")).unwrap(), std::fs::read(b.join("3.svmap")).unwrap());
    assert_eq!(snapshot(b.path()), other_before, "the other folder is never written");
}

/// A map only the other folder has: Copy into this folder makes a new unsaved tab; saving writes it into this folder, verified.
#[test]
fn folder_diff_copy_and_read_only() {
    let (a, b) = folders_ab("fdc");
    let mut h = Harness::new(None, StartOptions { folder: Some(a.path().to_path_buf()), ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_compare_folders();
    h.app.r5.fd.other = b.path().display().to_string();
    scan_done(&mut h);
    h.app.copy_into_folder(9);
    h.frames(2);
    let uid = {
        let d = &h.app.docs[h.app.active];
        assert_eq!((d.map_id, d.path.is_none(), d.is_dirty(), d.read_only), (Some(9), true, true, false));
        assert_eq!(shaiya_svmap::write(&d.map), std::fs::read(b.join("9.svmap")).unwrap());
        d.uid
    };
    assert_eq!(h.app.r5.showing, Showing::Map);
    h.app.begin_save_ext(uid, Some(a.join("9.svmap")), true, true);
    assert_eq!(std::fs::read(a.join("9.svmap")).unwrap(), std::fs::read(b.join("9.svmap")).unwrap());
    assert!(h.app.folder.as_ref().unwrap().maps.contains_key(&9), "the new map joins the folder");
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.contains("saved and verified")), "{:?}", h.app.toast);
    assert!(!h.app.docs[h.app.active].is_dirty() && h.app.docs[h.app.active].verified_at.is_some());
    h.frames(2);
    assert!(h.text.contains("Saved and verified"), "{}", h.text);
    // read-only copy of a map of the other folder
    h.app.open_other_read_only(2);
    let d = &h.app.docs[h.app.active];
    assert!(d.read_only && d.map_id == Some(2) && d.path.as_deref() == Some(b.join("2.svmap").as_path()));
}

/// The World tab: cards with their numbers, links, the Inspector's map card, drag and double-click, positions kept per folder.
#[test]
fn world_tab_cards_links_and_drag() {
    let dir = TempDir::new("world-tab");
    let data = TempDir::new("world-data");
    let maps = dir.path();
    three_maps(maps);
    let edit = |id: u32, f: &dyn Fn(&mut Svmap)| {
        let p = maps.join(format!("{id}.svmap"));
        let mut m = read_map(&p);
        f(&mut m);
        std::fs::write(&p, shaiya_svmap::write(&m)).unwrap();
    };
    edit(1, &|m| m.portals.extend([portal_to(2), portal_to(2), portal_to(77), portal_to(200)]));
    edit(2, &|m| m.portals.push(portal_to(1)));
    edit(3, &|m| m.portals.push(portal_to(3)));
    let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(maps.to_path_buf()), ..Default::default() });
    h.app.settings.first_run_done = true;
    wait_for_folder(&mut h);
    // no folder: a toast
    {
        let mut h0 = Harness::new(None, StartOptions::default());
        assert!(h0.app.can_run(Cmd::WorldView).is_err() && h0.app.can_run(Cmd::CompareFolders).is_err());
        h0.app.open_world();
        assert!(h0.app.toast.as_ref().is_some_and(|(t, _)| t.contains("Open a map folder first")));
        h0.frames(1);
    }
    h.app.open_file(&maps.join("1.svmap"));
    h.frames(2);
    let ctx = h.ctx.clone();
    h.app.run(Cmd::WorldView, &ctx);
    h.frames(4);
    assert_eq!(h.app.r5.showing, Showing::World);
    wait_until(&mut h, "the thumbnails of the three maps", WAIT, |h| h.app.r5.world.thumb_count() == 3);
    h.frames(2);
    assert_eq!(h.app.r5.world.thumb_count(), 3, "a picture of the blocked grid for every map");
    assert_eq!(h.app.r5.world.drawn_cards, 3);
    // links: 1<->2 (two way, 3 portals), 1->77 (missing), 1->200 (guild ranking), 3->3 (a loop)
    assert_eq!(h.app.r5.world.drawn_lines, 4);
    assert!(h.text.contains("1  Map 1") && h.text.contains("3 areas") && h.text.contains("monsters") && h.text.contains("4 out"), "{}", h.text);
    assert!(h.text.contains("map 77 (missing)") && h.text.contains("guild ranking"), "{}", h.text);
    // the Inspector shows the selected card's map
    h.app.r5.world.selected = Some(2);
    h.frames(2);
    assert!(h.text.contains("2  Map 2") && h.text.contains("Portals out") && h.text.contains("Open map"), "{}", h.text);
    // the open map is not edited from the keyboard or the menus while the tab shows
    h.app.docs[0].selection = vec![(SectionId::MobAreas, 0)];
    assert!(h.app.can_run(Cmd::Delete).is_err() && h.app.can_run(Cmd::Paste).is_err() && h.app.can_run(Cmd::SelectAll).is_err());
    let before = h.app.docs[0].map.clone();
    h.key(egui::Key::Delete, Modifiers::NONE);
    h.key(egui::Key::ArrowRight, Modifiers::NONE);
    h.key(egui::Key::Home, Modifiers::NONE);
    assert_eq!(h.app.docs[0].map, before);
    assert!(h.app.docs[0].undo.is_empty());
    assert_eq!(h.app.r5.showing, Showing::World, "keys do not leave the World tab");
    // an unsaved edit of an open tab counts on its card (live), with the unsaved dot
    h.app.docs[0].commit("add area", vec![Op::insert(3, mob_area(30.0, &[(5, 1)]))]);
    h.frames(2);
    assert!(h.text.contains("4 areas"), "{}", h.text);
    assert!(h.app.r5.world.drawn_cards == 3);
    h.app.docs[0].undo();
    h.frames(2);
    assert!(!h.text.contains("4 areas"), "{}", h.text);

    // drag card 1: the position moves and is kept for this folder
    let rect = h.app.r5.world.canvas.unwrap();
    let (zoom, pan) = (h.app.r5.world.zoom, h.app.r5.world.pan);
    let pos0 = h.app.r5.world.positions[&1];
    let centre = Pos2::new(rect.min.x + (pos0[0] + 90.0 - pan[0]) * zoom, rect.min.y + (pos0[1] + 115.0 - pan[1]) * zoom);
    h.drag(centre, centre + Vec2::new(60.0, 40.0));
    let pos1 = h.app.r5.world.positions[&1];
    assert!(pos1[0] > pos0[0] + 20.0 && pos1[1] > pos0[1] + 10.0, "{pos0:?} -> {pos1:?}");
    let key = crate::settings::path_key(&h.app.settings.svmap_folder.clone().unwrap());
    assert_eq!(h.app.settings.world_layout[&key]["1"], pos1);
    assert_eq!(h.app.settings.world_layout.len(), 1);
    h.app.flush_settings();
    // a new start finds it again
    {
        let mut h2 = Harness::new(Some(data.path()), StartOptions::default());
        wait_for_folder(&mut h2);
        h2.app.open_world();
        h2.frames(3);
        assert_eq!(h2.app.r5.world.positions[&1], pos1);
    }
    // double-click opens the map and shows its tab
    let c2 = {
        let (zoom, pan) = (h.app.r5.world.zoom, h.app.r5.world.pan);
        let p = h.app.r5.world.positions[&3];
        Pos2::new(rect.min.x + (p[0] + 90.0 - pan[0]) * zoom, rect.min.y + (p[1] + 115.0 - pan[1]) * zoom)
    };
    h.click(c2);
    assert_eq!(h.app.r5.world.selected, Some(3));
    let down = |pressed| Event::PointerButton { pos: c2, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.frame(vec![down(true)]);
    h.frame(vec![down(false)]);
    h.frames(2);
    assert_eq!(h.app.r5.showing, Showing::Map, "double-click shows the map");
    assert_eq!(h.app.docs[h.app.active].map_id, Some(3));
}

/// By links twice gives the same cards; By id puts them back in rows; both are kept per folder.
#[test]
fn world_arrange_deterministic() {
    let dir = TempDir::new("world-arrange");
    three_maps(dir.path());
    let mut h = Harness::new(None, StartOptions { folder: Some(dir.path().to_path_buf()), ..Default::default() });
    wait_for_folder(&mut h);
    h.app.open_world();
    h.frames(3);
    let grid = h.app.r5.world.positions.clone();
    h.app.apply_arrange(r5_ui::Arrange::ByLinks);
    let a = h.app.r5.world.positions.clone();
    h.app.apply_arrange(r5_ui::Arrange::ByLinks);
    assert_eq!(a, h.app.r5.world.positions);
    h.app.apply_arrange(r5_ui::Arrange::ById);
    assert_eq!(h.app.r5.world.positions, grid);
    // arranging when positions were saved asks first
    assert!(h.app.world_has_saved());
}

/// Every map edit of the Take buttons leaves a read-only tab alone.
#[test]
fn read_only_refuses_take() {
    let (a, b) = folders_ab("rot");
    let mut h = Harness::new(None, StartOptions::default());
    h.app.settings.other_folder = Some(b.path().display().to_string());
    h.app.settings.compare_against = CompareAgainst::Other;
    h.app.settings.layers.compare = true;
    h.app.open_read_only(&a.join("1.svmap"), None, Some(1));
    h.app.open_dock(DockTab::Changes);
    h.frames(3);
    let uid = h.app.docs[0].uid;
    let before = h.app.docs[0].map.clone();
    let rows = h.app.safety.cmp.get(&uid).unwrap().rows.clone();
    assert!(!rows.is_empty() && h.app.safety.cmp.get(&uid).unwrap().cells() > 0);
    h.app.compare_act(uid, true, safety::CAct::Revert(rows[0]));
    h.app.compare_act(uid, true, safety::CAct::Section(SectionId::MobAreas));
    h.app.compare_act(uid, true, safety::CAct::Cells);
    h.app.compare_act(uid, true, safety::CAct::Whole);
    h.frames(2);
    assert_eq!(h.app.docs[0].map, before);
    assert!(h.app.docs[0].undo.is_empty() && !h.app.docs[0].is_dirty());
    assert!(h.app.dialog.is_none(), "a read-only tab refuses before it asks");
    // the same on an editable tab does edit it (the test is not vacuous)
    h.app.open_file(&a.join("1.svmap"));
    h.frames(3);
    let uid2 = h.app.docs[h.app.active].uid;
    h.app.compare_act(uid2, true, safety::CAct::Whole);
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1);
}

/// A save through the app says "saved and verified".
#[test]
fn app_save_says_verified() {
    let dir = TempDir::new("app-verified");
    let f = svmap_file(dir.path(), 4, 64);
    let mut h = Harness::new(None, StartOptions { files: vec![f], ..Default::default() });
    h.frames(3);
    let uid = h.app.docs[0].uid;
    h.app.docs[0].commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
    h.app.begin_save(uid, None, true);
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.starts_with("saved and verified")), "{:?}", h.app.toast);
    h.frames(2);
    assert!(h.text.contains("Saved and verified"), "{}", h.text);
}

/// The folder-wide Problems list gets a "Server and client" group when the map folder's parent holds `Map.ini`, and the target
/// server setting changes the severity of a missing map file. Without a server data folder the checks stay off.
#[test]
fn problems_folder_shows_the_server_and_client_group() {
    let root = TempDir::new("c0-gui");
    let maps = root.join("Map");
    std::fs::create_dir_all(&maps).unwrap();
    svmap_file(&maps, 0, 64);
    svmap_file(&maps, 1, 64);
    std::fs::write(root.join("Map.ini"), b"[BASE]\r\nMapCount = 3\r\n").unwrap();
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.clone()), ..Default::default() });
    h.app.settings.problems_scope = crate::settings::ProblemsScope::Folder;
    h.app.settings.layout.open_dock(DockTab::Problems);
    wait_for_folder(&mut h);
    h.app.open_map_id(0);
    let wait_result = |h: &mut Harness| {
        wait_until(h, "the folder check", WAIT, |h| h.app.safety.folder_check.result.is_some());
        h.frames(3);
    };
    wait_result(&mut h);
    let r = h.app.safety.folder_check.result.as_ref().expect("the folder check ran");
    let missing: Vec<_> = r.cross.iter().filter(|c| c.finding.code == "mapini-no-svmap").collect();
    assert_eq!(missing.len(), 1, "id 2 is below MapCount 3 and has no file: {:?}", r.cross.iter().map(|c| c.finding.code).collect::<Vec<_>>());
    assert_eq!(missing[0].finding.severity, shaiya_svmap::Severity::Error);
    assert!(h.text.contains("Server and client") && h.text.contains("mapini-no-svmap"), "{}", h.text);
    assert!(r.totals().0 >= 1, "the cross findings count in the totals");
    // the target server changes the severity; the Preferences change makes the check run again
    h.app.settings.target_server = crate::settings::TargetServer::Afterglow;
    h.app.safety.invalidate();
    wait_result(&mut h);
    let r = h.app.safety.folder_check.result.as_ref().unwrap();
    assert!(r.cross.iter().filter(|c| c.finding.code == "mapini-no-svmap").all(|c| c.finding.severity == shaiya_svmap::Severity::Info));
    // no Map.ini beside the folder and no setting: the group is off and a hint says so
    let alone = TempDir::new("c0-gui-alone");
    svmap_file(alone.path(), 0, 64);
    let mut h = Harness::new(None, StartOptions { folder: Some(alone.path().to_path_buf()), ..Default::default() });
    h.app.settings.problems_scope = crate::settings::ProblemsScope::Folder;
    h.app.settings.layout.open_dock(DockTab::Problems);
    wait_for_folder(&mut h);
    h.app.open_map_id(0);
    wait_result(&mut h);
    assert!(h.app.safety.folder_check.result.as_ref().unwrap().cross.is_empty());
    assert!(h.text.contains("No server data folder"), "{}", h.text);
}

/// A tiny client folder (extracted layout) with map 7 (a good `.wld`) and map 8 (one that does not write back), and the two
/// svmap files; returns (client, maps).
fn c1_folders(root: &TempDir) -> (PathBuf, PathBuf) {
    let client = client_dir(root);
    std::fs::write(client.join("world").join("7.wld"), crate::testutil::tiny_wld(64).to_bytes()).unwrap();
    std::fs::write(client.join("world").join("8.wld"), crate::testutil::not_lossless_wld(64)).unwrap();
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    svmap_file(&maps, 7, 64);
    svmap_file(&maps, 8, 64);
    (client, maps)
}

/// Frames until the active map's client data has loaded.
fn wait_assets(h: &mut Harness) {
    wait_until(h, "the client data of the map", WAIT, |h| h.app.current_assets().is_some());
    h.frames(2);
}

/// Tools > Client > Stage client map: the staging folder gets a verified copy, the map shows the "staged" chip, the client
/// folder is untouched, a folder inside the client is refused and a `.wld` that does not write back stays read-only.
#[test]
fn stage_client_map_and_chips() {
    let root = TempDir::new("c1-stage");
    let (client, maps) = c1_folders(&root);
    let out = root.join("out");
    let before = std::fs::read(client.join("world").join("7.wld")).unwrap();
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.clone()), client: Some(client.clone()), maps: vec![7], ..Default::default() });
    wait_for_folder(&mut h);
    wait_assets(&mut h);
    assert!(!h.text.contains("staged") && !h.text.contains("read-only"), "no chip before staging: {}", h.text);
    assert!(h.app.can_run(Cmd::StageClientMap).is_ok());
    // a staging folder inside the client is refused with the reason, and nothing is written
    h.app.settings.client_out = Some(client.join("staged").display().to_string());
    h.app.stage_client_map();
    assert!(matches!(&h.app.dialog, Some(Dialog::Error(e)) if e.contains("never writes into the client")), "refused");
    assert!(!client.join("staged").exists() && !client.join("data").exists());
    h.app.dialog = None;
    // a good folder: the file is written, verified, and the map reads it from there
    h.app.settings.client_out = Some(out.display().to_string());
    h.app.stage_client_map();
    assert!(h.app.dialog.is_none(), "no error dialog");
    let staged = out.join("data").join("world").join("7.wld");
    assert_eq!(std::fs::read(&staged).unwrap(), before, "no edit: the staged bytes are the original ones");
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.contains("staged and verified")), "{:?}", h.app.toast);
    h.frames(3);
    assert!(h.text.contains("staged"), "the chip shows: {}", h.text);
    assert!(h.app.client.as_ref().unwrap().wld_state(7).staged);
    assert_eq!(std::fs::read(client.join("world").join("7.wld")).unwrap(), before, "the client folder is untouched");
    assert!(!client.join("data").exists() && !client.join("world").join("7.wld.bak").exists());
    // staging again changes nothing
    h.app.stage_client_map();
    assert!(h.app.toast.as_ref().is_some_and(|(t, _)| t.contains("already staged")), "{:?}", h.app.toast);
    // clearing the staging folder in the Preferences brings the client's own copy back
    h.app.client_out_changed(None);
    h.app.toast = None;
    wait_assets(&mut h);
    assert!(!h.app.client.as_ref().unwrap().wld_state(7).staged && !h.text.contains("staged"), "{}", h.text);

    // map 8 does not write back: the chip and the Problems info say so, and staging refuses it
    h.app.open_map_id(8);
    h.frames(2);
    wait_assets(&mut h);
    assert!(h.text.contains("client map read-only"), "{}", h.text);
    let f = &h.app.docs[h.app.active].findings;
    assert!(f.iter().any(|f| f.code == "wld-not-lossless" && f.severity == Severity::Info), "{f:?}");
    h.app.stage_client_map();
    assert!(matches!(&h.app.dialog, Some(Dialog::Error(e)) if e.contains("stays read-only") && e.contains("first difference")), "{:?}", matches!(&h.app.dialog, Some(Dialog::Error(_))));
    assert!(!out.join("data").join("world").join("8.wld").exists());
}

/// The staging folder setting is checked when a client opens: one inside the client is dropped; one outside is used.
#[test]
fn staging_setting_checked_on_open() {
    let root = TempDir::new("c1-setting");
    let (client, maps) = c1_folders(&root);
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.clone()), maps: vec![7], ..Default::default() });
    h.app.settings.client_out = Some(client.join("inside").display().to_string());
    let ctx = h.ctx.clone();
    h.app.open_client(&client, &ctx);
    assert!(h.app.settings.client_out.is_none(), "a folder inside the client is dropped");
    assert!(h.app.client.as_ref().unwrap().staging().is_none());
    h.app.settings.client_out = Some(root.join("out").display().to_string());
    h.app.open_client(&client, &ctx);
    assert_eq!(h.app.client.as_ref().unwrap().staging(), Some(root.join("out")));
}

// ------------------------------------------------------------------------------------------------------------------ paired documents

/// A window over the paired folders (map 9 pairs with its `.wld`, map 10 does not write back) with the staging folder set.
fn c2_harness(root: &TempDir) -> (Harness, PathBuf, PathBuf) {
    let (client, maps) = crate::testutil::paired_folders(root);
    let out = root.join("out");
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.clone()), client: Some(client.clone()), maps: vec![9], ..Default::default() });
    h.app.settings.client_out = Some(out.display().to_string());
    wait_for_folder(&mut h);
    wait_assets(&mut h);
    h.frames(3);
    (h, client, out)
}

fn move_active_portal(h: &mut Harness, i: usize, dx: f32) {
    let d = &mut h.app.docs[h.app.active];
    let before = edit::get(&d.map, SectionId::Portals, i).unwrap();
    let mut after = before.clone();
    edit::translate(&mut after, dx, 0.0);
    d.commit("move portal", vec![Op::replace(i, before, after)]);
}

/// Ctrl+S writes the svmap through the normal save and then the staged `.wld` and text; the client folder is never written; the
/// chips say what is unsaved and what is staged.
#[test]
fn ctrl_s_saves_the_svmap_then_the_staged_client_files() {
    let root = TempDir::new("c2-save");
    let (mut h, client, out) = c2_harness(&root);
    assert!(h.app.docs[h.app.active].part().is_some(), "the tab has its client half");
    assert!(!h.text.contains("client map unsaved"));
    let before_wld = std::fs::read(client.join("world").join("9.wld")).unwrap();
    let before_txt = std::fs::read(client.join("world").join("9.txt")).unwrap();
    move_active_portal(&mut h, 1, 6.0);
    h.frames(2);
    assert!(h.text.contains("client map unsaved"), "{}", h.text);
    h.app.request_save(false);
    h.frames(2);
    assert!(h.app.dialog.is_none(), "no error");
    // the svmap file has the edit
    let saved = shaiya_svmap::read(&std::fs::read(h.app.docs[h.app.active].path.clone().unwrap()).unwrap()).unwrap();
    assert!((saved.portals[1].pos[0] - h.app.docs[h.app.active].map.portals[1].pos[0]).abs() < 1e-6);
    // the staged .wld has the box 6 units further, the client folder is as it was
    let staged = std::fs::read(out.join("data").join("world").join("9.wld")).unwrap();
    let raw = shaiya_clientdata::wld_raw::WldRaw::parse(&staged, "staged").unwrap();
    assert!((crate::pair::box_pos(&raw.portals[1])[0] - saved.portals[1].pos[0]).abs() < 1e-3);
    assert_eq!(std::fs::read(client.join("world").join("9.wld")).unwrap(), before_wld);
    assert_eq!(std::fs::read(client.join("world").join("9.txt")).unwrap(), before_txt);
    assert!(!client.join("data").exists());
    assert!(!h.app.docs[h.app.active].is_dirty() && h.app.docs[h.app.active].client_error.is_none());
    h.frames(3);
    assert!(!h.text.contains("client map unsaved") && h.text.contains("staged"), "{}", h.text);
    // a second edit that only renames a region writes the text and leaves the svmap file alone
    let svmap_file = h.app.docs[h.app.active].path.clone().unwrap();
    let stamp = std::fs::metadata(&svmap_file).unwrap().modified().unwrap();
    let txt = h.app.docs[h.app.active].part().unwrap().txt.clone();
    let after = crate::pair::txt_with_name(&txt, 0, "Renamed").unwrap();
    h.app.docs[h.app.active].commit_client("rename region #0", vec![crate::pair::COp::Txt { before: txt, after }]).unwrap();
    assert!(h.app.docs[h.app.active].client_dirty() && !h.app.docs[h.app.active].svmap_dirty());
    h.app.request_save(false);
    let names = shaiya_clientdata::area_txt::AreaTxt::parse(&std::fs::read(out.join("data").join("world").join("9.txt")).unwrap()).names();
    assert_eq!(names.get(0), Some("Renamed"));
    assert_eq!(std::fs::metadata(&svmap_file).unwrap().modified().unwrap(), stamp, "the svmap file was not written again");
    assert!(!h.app.docs[h.app.active].is_dirty());
}

/// placement edits make only the client half modified; R turns the selection, Ctrl+R toggles the rulers and
/// turns nothing; Ctrl+S writes the staged `.wld` with the new building and leaves the svmap file and the client folder alone.
#[test]
fn placement_edits_save_the_client_map_only() {
    let root = TempDir::new("c3-save");
    let (mut h, client, out) = c2_harness(&root);
    let before_wld = std::fs::read(client.join("world").join("9.wld")).unwrap();
    let svmap_file = h.app.docs[h.app.active].path.clone().unwrap();
    let stamp = std::fs::metadata(&svmap_file).unwrap().modified().unwrap();
    h.key(egui::Key::Num8, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Place);
    let n0 = h.app.docs[h.app.active].part().unwrap().wld.model.building.items.len();
    {
        let mut ui = crate::place_ui::PlaceUi::default();
        ui.armed = Some((crate::place::PlaceList::Building, "review.smod".to_string()));
        let d = &mut h.app.docs[h.app.active];
        crate::place_ui::add_at(d, &ui, [20.0, 20.0], None, 1.0, None, shaiya_clientdata::wld_raw::Codepage::Cp949).unwrap();
        assert!(d.client_dirty() && !d.svmap_dirty(), "a placement edit is a client edit only");
    }
    h.frames(2);
    let yaw = |h: &Harness| crate::place::yaw_deg(&h.app.docs[h.app.active].part().unwrap().wld.model.building.items[n0]);
    assert!(yaw(&h).abs() < 1e-3);
    let rulers = h.app.settings.rulers;
    h.key(egui::Key::R, Modifiers::COMMAND);
    h.frames(1);
    assert_ne!(h.app.settings.rulers, rulers, "Ctrl+R is Rulers");
    assert!(yaw(&h).abs() < 1e-3, "and does not turn: {}", yaw(&h));
    h.key(egui::Key::R, Modifiers::NONE);
    h.frames(1);
    assert!((yaw(&h) - 15.0).abs() < 1e-3, "R turns by 15: {}", yaw(&h));
    h.app.request_save(false);
    h.frames(2);
    assert!(h.app.dialog.is_none(), "no error");
    let raw = shaiya_clientdata::wld_raw::WldRaw::parse(&std::fs::read(out.join("data").join("world").join("9.wld")).unwrap(), "staged").unwrap();
    assert_eq!(raw.building.items.len(), n0 + 1);
    assert_eq!(crate::place::model_name(&raw.building, &raw.building.items[n0]).as_deref(), Some("review.smod"));
    assert_eq!(std::fs::read(client.join("world").join("9.wld")).unwrap(), before_wld, "the client folder is untouched");
    assert!(!client.join("data").exists());
    assert_eq!(std::fs::metadata(&svmap_file).unwrap().modified().unwrap(), stamp, "the svmap file was not written");
    assert!(!h.app.docs[h.app.active].is_dirty());
}

/// Save as to another map number with unsaved client edits: the edits go to the client map they were made on (`world/9.wld`),
/// never to the new number's. The tab then reads the client map of the new number, and undo leaves both client files alone.
#[test]
fn save_as_another_number_writes_the_client_map_the_edits_belong_to() {
    let root = TempDir::new("c2-save-as");
    let (mut h, client, out) = c2_harness(&root);
    // the client's map 50: map 9 with another destination on portal 1
    let (mut w50, _, _) = crate::testutil::paired_fixture(64);
    w50.portals[1].dest_map = 77;
    std::fs::write(client.join("world").join("50.wld"), w50.to_bytes()).unwrap();
    move_active_portal(&mut h, 1, 6.0);
    let target = root.join("maps").join("50.svmap");
    let uid = h.app.docs[h.app.active].uid;
    h.app.begin_save(uid, Some(target.clone()), false);
    h.frames(2);
    assert!(h.app.dialog.is_none(), "no error");
    assert!(target.is_file());
    let staged = out.join("data").join("world");
    assert!(staged.join("9.wld").is_file() && !staged.join("50.wld").exists());
    let wld9 = std::fs::read(staged.join("9.wld")).unwrap();
    h.frames(3);
    let d = &h.app.docs[h.app.active];
    assert_eq!(d.map_id, Some(50));
    assert_eq!(d.part().map(|p| p.wld.model.portals[1].dest_map), Some(77), "the tab holds the client's map 50 now");
    assert!(!d.is_dirty());
    let l50 = d.part().unwrap().lists();
    let d = &mut h.app.docs[h.app.active];
    d.undo();
    assert!(d.svmap_dirty() && !d.client_dirty());
    assert_eq!(d.part().unwrap().lists(), l50, "undo did not write map 9's portal into map 50");
    h.frames(2);
    assert_eq!(std::fs::read(staged.join("9.wld")).unwrap(), wld9);
    assert!(!staged.join("50.wld").exists());
}

/// A failed client write leaves the svmap saved and the client half modified, and the status bar says so.
#[test]
fn a_failed_client_save_keeps_the_client_half_modified() {
    let root = TempDir::new("c2-save-fail");
    let (mut h, client, _out) = c2_harness(&root);
    // a staging folder inside the client is refused by the same check as the stage command
    h.app.settings.client_out = Some(client.join("staged").display().to_string());
    move_active_portal(&mut h, 0, 2.0);
    let file = h.app.docs[h.app.active].path.clone().unwrap();
    let before = std::fs::read(&file).unwrap();
    h.app.request_save(false);
    h.frames(2);
    assert!(matches!(&h.app.dialog, Some(Dialog::Error(e)) if e.contains("never writes into the client")), "refused");
    assert_ne!(std::fs::read(&file).unwrap(), before, "the svmap was saved");
    let d = &h.app.docs[h.app.active];
    assert!(!d.svmap_dirty() && d.client_dirty() && d.is_dirty() && d.client_error.is_some());
    assert!(!client.join("staged").exists() && !client.join("data").exists());
    h.app.dialog = None;
    h.frames(2);
    assert!(h.text.contains("client map not saved"), "{}", h.text);
    // with a good folder the next Ctrl+S writes it
    let out = root.join("out2");
    h.app.settings.client_out = Some(out.display().to_string());
    h.app.request_save(false);
    assert!(out.join("data").join("world").join("9.wld").is_file());
    assert!(!h.app.docs[h.app.active].is_dirty() && h.app.docs[h.app.active].client_error.is_none());
}

/// A map whose `.wld` does not write back byte for byte gets no client half: its edits stay on the svmap and client edits are
/// refused with the reason.
#[test]
fn a_read_only_client_map_refuses_client_edits_in_the_window() {
    let root = TempDir::new("c2-ro-window");
    let (mut h, _client, out) = c2_harness(&root);
    h.app.open_map_id(10);
    h.frames(3);
    wait_assets(&mut h);
    let d = &mut h.app.docs[h.app.active];
    assert_eq!(d.map_id, Some(10));
    assert!(matches!(d.client, crate::doc::ClientSlot::ReadOnly(_)));
    let err = d.commit_client("rename", vec![crate::pair::COp::Txt { before: Vec::new(), after: crate::testutil::utf16_bom("0\tX\r\n") }]).unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    d.commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
    assert!(d.svmap_dirty() && !d.client_dirty());
    h.app.request_save(false);
    assert!(!out.join("data").join("world").join("10.wld").exists(), "nothing is staged for a read-only client map");
}

/// A paired section asks nothing when a portal is added, moved in the list or deleted; an unpaired one still asks first.
#[test]
fn only_unpaired_sections_ask_before_portal_changes() {
    let root = TempDir::new("c2-confirm");
    let (mut h, _client, _out) = c2_harness(&root);
    let n = h.app.docs[h.app.active].map.portals.len();
    h.app.request(Pending::AddPortal([5.0, 0.0, 5.0]));
    assert!(h.app.dialog.is_none(), "paired: no question");
    let d = &h.app.docs[h.app.active];
    assert_eq!((d.map.portals.len(), d.part().unwrap().count(SectionId::Portals)), (n + 1, n + 1));
    h.app.request(Pending::MovePortal(0, false));
    assert!(h.app.dialog.is_none());
    h.app.docs[h.app.active].select_only(SectionId::Portals, 0);
    h.app.request(Pending::Delete);
    assert!(h.app.dialog.is_none());
    assert_eq!(h.app.docs[h.app.active].map.portals.len(), n);
    // an unpaired portal list asks
    let d = &mut h.app.docs[h.app.active];
    let r = edit::get(&d.map, SectionId::Portals, 0).unwrap();
    d.commit_server_only("drop one on the svmap", vec![Op::remove(0, r)]);
    h.app.request(Pending::AddPortal([5.0, 0.0, 5.0]));
    assert!(matches!(&h.app.dialog, Some(Dialog::Confirm { .. })), "unpaired: the old question");
}

/// Unsaved client edits come back from the recovery folder with the tab, and the entry stays until the tab is saved.
#[test]
fn recovery_keeps_the_client_half() {
    let root = TempDir::new("c2-recovery");
    let data = TempDir::new("c2-recovery-data");
    let (client, maps) = crate::testutil::paired_folders(&root);
    let opts = || StartOptions { folder: Some(maps.clone()), client: Some(client.clone()), maps: vec![9], ..Default::default() };
    let l_edited;
    {
        let mut h = Harness::new(Some(data.path()), opts());
        wait_for_folder(&mut h);
        wait_assets(&mut h);
        h.frames(3);
        move_active_portal(&mut h, 2, 4.0);
        let txt = h.app.docs[h.app.active].part().unwrap().txt.clone();
        let after = crate::pair::txt_with_name(&txt, 1, "Recovered").unwrap();
        h.app.docs[h.app.active].commit_client("rename", vec![crate::pair::COp::Txt { before: txt, after }]).unwrap();
        l_edited = h.app.docs[h.app.active].part().unwrap().lists();
        h.app.autosave_tick(true);
        let key = recovery::entry_key(h.app.docs[h.app.active].path.as_deref(), h.app.docs[h.app.active].uid);
        let dir = recovery::dir(data.path());
        assert!(dir.join(format!("{key}.wld")).is_file() && dir.join(format!("{key}.txt")).is_file(), "the client files are in the entry");
        // the window "crashes": nothing is removed
    }
    let mut h = Harness::new(Some(data.path()), opts());
    let found = recovery::scan(data.path(), u64::MAX / 2, 1);
    assert_eq!(found.len(), 1);
    assert!(found[0].meta.client && found[0].client.is_some());
    h.app.restore_recovered(found.into_iter().next().unwrap()).unwrap();
    h.frames(3);
    let d = &h.app.docs[h.app.active];
    assert_eq!(d.part().unwrap().lists(), l_edited, "both halves came back");
    assert!(d.client_dirty());
    assert_eq!(d.part().unwrap().names().get(1), Some("Recovered"));
    // undo of the recovery step gives the client as it is on disk
    let mut d2 = h.app.docs.pop().unwrap();
    while d2.undo().is_some() {}
    assert!(!d2.client_dirty(), "undo returns to the client's own file");
    assert_eq!(d2.part().unwrap().names().get(1), Some("Beta"));
}

/// Without the client folder a recovered entry that holds client edits is refused (nothing is lost) and the entry stays.
#[test]
fn recovery_with_client_edits_needs_the_client_folder() {
    let root = TempDir::new("c2-recovery-noclient");
    let data = TempDir::new("c2-recovery-noclient-data");
    let (client, maps) = crate::testutil::paired_folders(&root);
    {
        let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(maps.clone()), client: Some(client.clone()), maps: vec![9], ..Default::default() });
        wait_for_folder(&mut h);
        wait_assets(&mut h);
        h.frames(3);
        move_active_portal(&mut h, 0, 1.0);
        h.app.autosave_tick(true);
    }
    let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(maps.clone()), ..Default::default() });
    let found = recovery::scan(data.path(), u64::MAX / 2, 1);
    let e = found.into_iter().next().unwrap();
    let err = h.app.restore_recovered(e).unwrap_err();
    assert!(err.contains("open the client folder"), "{err}");
    assert!(h.app.docs.is_empty());
    assert_eq!(recovery::scan(data.path(), u64::MAX / 2, 1).len(), 1, "the entry is still there");
}

/// Autosave writes the client files of an entry only when the client half changed: an edit of the svmap alone leaves the 5 MB
/// `.wld` of a field map as it is. A clean client half takes its files out of the entry.
#[test]
fn autosave_writes_the_client_files_only_when_they_changed() {
    let root = TempDir::new("c2-autosave");
    let data = TempDir::new("c2-autosave-data");
    let (client, maps) = crate::testutil::paired_folders(&root);
    let mut h = Harness::new(Some(data.path()), StartOptions { folder: Some(maps.clone()), client: Some(client.clone()), maps: vec![9], ..Default::default() });
    wait_for_folder(&mut h);
    wait_assets(&mut h);
    h.frames(3);
    move_active_portal(&mut h, 0, 2.0);
    h.app.autosave_tick(true);
    let key = recovery::entry_key(h.app.docs[h.app.active].path.as_deref(), h.app.docs[h.app.active].uid);
    let wld = recovery::dir(data.path()).join(format!("{key}.wld"));
    assert!(wld.is_file());
    // mark the file: a rewrite would replace the mark
    std::fs::write(&wld, b"mark").unwrap();
    let d = &mut h.app.docs[h.app.active];
    let before = edit::get(&d.map, SectionId::Portals, 1).unwrap();
    let mut after = before.clone();
    edit::translate(&mut after, 1.0, 0.0);
    d.commit_server_only("svmap only", vec![Op::replace(1, before, after)]);
    h.app.autosave_tick(true);
    assert_eq!(std::fs::read(&wld).unwrap(), b"mark", "the svmap edit did not write the client files again");
    move_active_portal(&mut h, 2, 2.0);
    h.app.autosave_tick(true);
    let model = h.app.docs[h.app.active].part().unwrap().wld.model.to_bytes();
    assert_eq!(std::fs::read(&wld).unwrap(), model, "a client edit did");
    // undo everything and change the svmap alone: the client half is clean again and its files leave the entry
    let d = &mut h.app.docs[h.app.active];
    while d.undo().is_some() {}
    let before = edit::get(&d.map, SectionId::Portals, 1).unwrap();
    let mut after = before.clone();
    edit::translate(&mut after, 1.0, 0.0);
    d.commit_server_only("svmap only", vec![Op::replace(1, before, after)]);
    assert!(!d.client_dirty() && d.is_dirty());
    h.app.autosave_tick(true);
    assert!(!wld.exists(), "no stray client files");
    assert!(recovery::dir(data.path()).join(format!("{key}.svmap")).is_file());
}

/// The Changes tab lists what the client half changed.
#[test]
fn changes_tab_lists_client_changes() {
    let root = TempDir::new("c2-changes");
    let (mut h, _client, _out) = c2_harness(&root);
    move_active_portal(&mut h, 0, 3.0);
    let lines = h.app.docs[h.app.active].part().unwrap().changes();
    assert_eq!(lines, vec!["client portals: 1 changed".to_string()]);
    h.app.open_dock(DockTab::Changes);
    h.frames(3);
    assert!(h.text.contains("client portals: 1 changed"), "{}", h.text);
}

/// The inspector shows the chain row for a paired section and says why an unpaired one is not linked; the Sync window offers the
/// direction of each row when the tab has a client half.
#[test]
fn chain_rows_and_the_two_way_sync_window() {
    let root = TempDir::new("c2-ui");
    let (mut h, _client, _out) = c2_harness(&root);
    h.app.docs[h.app.active].select_only(SectionId::Portals, 0);
    h.frames(3);
    assert!(h.text.contains("linked to the client map") && h.text.contains("client destination name"), "{}", h.text);
    h.app.docs[h.app.active].select_only(SectionId::Regions, 1);
    h.frames(3);
    assert!(h.text.contains("client name") && h.text.contains("Beta"), "{}", h.text);
    // an unpaired portal list says so
    let d = &mut h.app.docs[h.app.active];
    let r = edit::get(&d.map, SectionId::Portals, 2).unwrap();
    d.commit_server_only("drop a portal on the svmap", vec![Op::remove(2, r)]);
    d.select_only(SectionId::Portals, 0);
    h.frames(3);
    assert!(h.text.contains("not linked: the client map has 3 here"), "{}", h.text);
    h.app.open_sync();
    h.frames(3);
    assert!(h.text.contains("all take server") && h.text.contains("take client"), "{}", h.text);
}

// ---------------------------------------------------------------------------------------------------------------- terrain

/// A terrain stroke on a tab: the tool key selects it; the stroke is live, one undo step at its end; the tab's picture and ground
/// follow the heights; Ctrl+S writes the staged `.wld` (heights only changed) and leaves the svmap file and the client folder alone.
#[test]
fn terrain_edits_redraw_the_picture_and_save_the_client_map_only() {
    let root = TempDir::new("c4-save");
    let (mut h, client, out) = c2_harness(&root);
    let before_wld = std::fs::read(client.join("world").join("9.wld")).unwrap();
    let svmap_file = h.app.docs[h.app.active].path.clone().unwrap();
    let stamp = std::fs::metadata(&svmap_file).unwrap().modified().unwrap();
    h.key(egui::Key::Num9, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Terrain, "9 is the Terrain tool");
    assert!(h.app.tool_blocked(Tool::Terrain).is_none());
    let ground0 = h.app.current_assets().unwrap().ground.height(20.0, 20.0).unwrap();
    {
        let t = &mut h.app.terrain;
        t.keep_on_ground = false; // records that stand on the ground would change the svmap too (that is tested with the re-drop)
        t.params = crate::terrain::Params { brush: crate::terrain::Brush::Set, radius: 8.0, falloff: 0.0, amount: 1.0, height: 30.0 };
        let d = &mut h.app.docs[h.app.active];
        t.begin(d, [20.0, 20.0], false).unwrap();
        assert!(!d.client_dirty(), "a stroke in progress is no step yet");
        let rep = t.finish(d).unwrap().unwrap();
        assert!(rep.corners > 10);
        assert!(d.client_dirty() && !d.svmap_dirty());
    }
    h.frames(3);
    let d = &h.app.docs[h.app.active];
    assert_eq!(d.undo.len(), 1);
    let uid = d.uid;
    let g = h.app.current_assets().unwrap().ground.height(20.0, 20.0).unwrap();
    assert!((g - 30.0).abs() < 0.05 && (g - ground0).abs() > 10.0, "the tab's ground follows the edit: {ground0} -> {g}");
    assert!(h.app.terrain_picture(h.app.active).is_some(), "the tab has its own redrawn picture");
    assert!(h.app.terr_views.contains_key(&uid));
    // the shared picture of the map is not changed: another tab of this map would still show the file's terrain
    let base = h.app.ready_for(Some(9)).unwrap().assets.ground.height(20.0, 20.0).unwrap();
    assert!((base - ground0).abs() < 1e-4);
    h.app.request_save(false);
    h.frames(2);
    assert!(h.app.dialog.is_none(), "no error");
    let staged = std::fs::read(out.join("data").join("world").join("9.wld")).unwrap();
    assert_eq!(staged.len(), before_wld.len(), "the length is exact");
    let raw = shaiya_clientdata::wld_raw::WldRaw::parse(&staged, "staged").unwrap();
    let t = raw.terrain.as_ref().unwrap();
    assert_eq!(t.heights[10 * t.corners() + 10], crate::terrain::raw_of(30.0));
    assert!(raw.validate().is_empty());
    assert_eq!(std::fs::read(client.join("world").join("9.wld")).unwrap(), before_wld, "the client folder is untouched");
    assert!(!client.join("data").exists());
    assert_eq!(std::fs::metadata(&svmap_file).unwrap().modified().unwrap(), stamp, "the svmap file was not written");
    assert!(!h.app.docs[h.app.active].is_dirty());
    // undo brings the ground back (the view follows) and marks the client map modified again
    h.app.docs[h.app.active].undo();
    h.frames(3);
    let g2 = h.app.current_assets().unwrap().ground.height(20.0, 20.0).unwrap();
    assert!((g2 - ground0).abs() < 1e-4, "undo gives the ground back: {g2}");
    assert!(h.app.docs[h.app.active].client_dirty());
}

/// The tool is disabled with the reason on a tab without an editable client map, and it refuses when the stroke would write nothing.
#[test]
fn terrain_tool_is_blocked_with_a_reason() {
    let root = TempDir::new("c4-blocked");
    let (mut h, _client, _out) = c2_harness(&root);
    assert!(h.app.tool_blocked(Tool::Terrain).is_none());
    // a dungeon: no terrain
    h.app.docs[h.app.active].part_mut().unwrap().wld.model.terrain = None;
    assert_eq!(h.app.tool_blocked(Tool::Terrain), Some("dungeon map: no terrain"));
    // a read-only tab
    h.app.docs[h.app.active].read_only = true;
    assert_eq!(h.app.tool_blocked(Tool::Terrain), Some("read-only tab"));
    h.app.docs[h.app.active].read_only = false;
    // no client map
    h.app.docs[h.app.active].set_client(crate::doc::ClientSlot::None);
    assert!(h.app.tool_blocked(Tool::Terrain).unwrap().contains("client"));
    h.app.docs[h.app.active].set_client(crate::doc::ClientSlot::ReadOnly("not lossless".to_string()));
    assert_eq!(h.app.tool_blocked(Tool::Terrain), Some("the client map is read-only"));
    // the key does not select a blocked tool's effect: the palette shows the reason and the tab stays editable by the others
    h.frames(2);
}

/// A stroke whose tab gets an undo, a new step or another client half while the button is down is cancelled and writes nothing
/// into the history  (the rule: no client op meets another client half).
#[test]
fn a_terrain_stroke_never_survives_a_new_client_half() {
    let root = TempDir::new("c4-half");
    let (mut h, _client, _out) = c2_harness(&root);
    {
        let t = &mut h.app.terrain;
        t.params = crate::terrain::Params { brush: crate::terrain::Brush::Set, radius: 8.0, falloff: 0.0, amount: 1.0, height: 30.0 };
        let d = &mut h.app.docs[h.app.active];
        t.begin(d, [20.0, 20.0], false).unwrap();
    }
    h.app.attach_client_now(h.app.docs[h.app.active].uid);
    h.frames(2);
    assert!(h.app.terrain.stroke.is_none(), "the stroke was cancelled");
    let d = &h.app.docs[h.app.active];
    assert!(d.undo.is_empty() && !d.client_dirty());
    let t = d.part().unwrap().terrain().unwrap();
    assert!(t.heights.iter().all(|v| *v == 9000), "the new half holds the file's heights");
}

/// the window's side of a stroke in progress. A save while the button is down writes the steps only (the drawn
/// corners are put back first, so the staged file and the "saved" state agree); switching the tool with the button down cancels the stroke; a stroke whose release the view never saw ends as its step on the
/// next frame.
#[test]
fn review_a_stroke_in_progress_never_reaches_a_file() {
    let root = TempDir::new("c4-midstroke");
    let (mut h, _client, out) = c2_harness(&root);
    h.key(egui::Key::Num9, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Terrain);
    let set = |h: &mut Harness, height: f32, at: [f32; 2]| {
        let t = &mut h.app.terrain;
        t.keep_on_ground = false;
        t.params = crate::terrain::Params { brush: crate::terrain::Brush::Set, radius: 6.0, falloff: 0.0, amount: 1.0, height };
        let d = &mut h.app.docs[h.app.active];
        t.begin(d, at, false).unwrap();
    };
    let corner = |h: &Harness, cx: usize, cz: usize| {
        let t = h.app.docs[h.app.active].part().unwrap().terrain().unwrap();
        t.heights[cz * t.corners() + cx]
    };
    // a stroke whose release was not seen: the next frame (button up) makes it the step
    set(&mut h, 30.0, [20.0, 20.0]);
    h.frames(1);
    assert!(h.app.terrain.stroke.is_none());
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1, "the missed release ended the stroke as one step");
    assert_eq!(corner(&h, 10, 10), crate::terrain::raw_of(30.0));
    // a save while a second stroke is drawn: the second stroke is cancelled, the file holds the first one only
    set(&mut h, 60.0, [40.0, 40.0]);
    assert_eq!(corner(&h, 20, 20), crate::terrain::raw_of(60.0), "drawn live");
    let uid = h.app.docs[h.app.active].uid;
    assert!(h.app.save_client_part(uid));
    assert!(h.app.terrain.stroke.is_none(), "the save cancelled the stroke");
    assert_eq!(corner(&h, 20, 20), 9000, "the drawn corners are back");
    let staged = std::fs::read(out.join("data").join("world").join("9.wld")).unwrap();
    let raw = shaiya_clientdata::wld_raw::WldRaw::parse(&staged, "staged").unwrap();
    assert_eq!(raw.terrain.as_ref().unwrap().heights, h.app.docs[h.app.active].part().unwrap().terrain().unwrap().heights, "the staged file is the model");
    assert!(!h.app.docs[h.app.active].client_dirty());
    // switching the tool with the button down cancels the stroke (the guard sees another tool on the next frame)
    set(&mut h, 60.0, [40.0, 40.0]);
    assert!(h.app.terrain.stroking(uid));
    h.app.tool = Tool::Select;
    h.frames(1);
    assert!(h.app.terrain.stroke.is_none());
    assert_eq!(corner(&h, 20, 20), 9000);
    assert_eq!(h.app.docs[h.app.active].undo.len(), 1, "no step from the cancelled stroke");
    assert!(!h.app.docs[h.app.active].part().unwrap().has_live());
}

// ------------------------------------------------------------------------------------------------------------------ Map.ini and new maps

const C5_MAP_INI: &[u8] = b"[BASE]\r\nMapCount\t  = 11\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\nWeatherState = 0\r\n\r\n[SET_ZONE_9]\r\nMapType = F\t// \xB0\xA1\r\n\r\n[SET_ZONE_10]\r\nMapType = D\r\nRebirthMapPos = 9, 1, 2, 3\r\n\r\n";
const C5_MAP_WAR: &[u8] = b"[BASE]\r\nMapCount = 1\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\n\r\n[SET_ZONE_100]\r\nMapType = D\r\n";

/// A window over the paired folders with a server data folder (`Map.ini` MapCount 11, zones 9 and 10, `MapWar.ini` with zone 100)
/// beside the map folder, a staging folder and a data folder.
fn c5_harness(root: &TempDir) -> (Harness, PathBuf, PathBuf) {
    let (client, maps) = crate::testutil::paired_folders(root);
    std::fs::write(root.join("Map.ini"), C5_MAP_INI).unwrap();
    std::fs::write(root.join("MapWar.ini"), C5_MAP_WAR).unwrap();
    let data = root.join("data-dir");
    std::fs::create_dir_all(&data).unwrap();
    let mut h = Harness::new(Some(&data), StartOptions { folder: Some(maps.clone()), client: Some(client.clone()), maps: vec![9], ..Default::default() });
    h.app.settings.client_out = Some(root.join("out").display().to_string());
    wait_for_folder(&mut h);
    wait_assets(&mut h);
    h.frames(3);
    (h, client, maps)
}

/// The Map.ini editor window: the files open, a form edit is one undo step and only changes its lines, Save writes the file in
/// the server data folder after a copy of the old one, a zone that the rules refuse is not added, and "pick on map" writes a
/// rebirth position from a click.
#[test]
fn c5_map_ini_editor_window() {
    let root = TempDir::new("c5-ini-window");
    let (mut h, client, maps) = c5_harness(&root);
    assert!(h.app.can_run(Cmd::MapIniEditor).is_ok() && h.app.can_run(Cmd::AddMap).is_ok());
    let client_before = std::fs::read(client.join("world").join("9.wld")).unwrap();
    h.app.open_ini_editor();
    h.frames(3);
    for want in ["Map.ini editor", "MapWar.ini", "MapCount 11", "Zone 9 (field)", "Rebirth", "Weather", "Add a zone", "next id: 11"] {
        assert!(h.text.contains(want), "{want}: {}", h.text);
    }
    // a form edit: one step, dirty, only its lines change
    let before = std::fs::read(root.join("Map.ini")).unwrap();
    let mut w = h.app.c5.ini.take().unwrap();
    h.app.ini_act(&mut w, IniAct::Set(9, "MapType".to_string(), "D".to_string()));
    h.app.ini_act(&mut w, IniAct::Set(9, "WeatherState".to_string(), "2".to_string()));
    assert!(w.docs[0].dirty() && !w.docs[1].dirty());
    h.app.ini_act(&mut w, IniAct::Set(9, "WeatherState".to_string(), "x // y".to_string()));
    assert!(matches!(&w.msg, Some((true, m)) if m.contains("//")), "{:?}", w.msg);
    h.app.c5.ini = Some(w);
    h.frames(2);
    assert!(h.text.contains("Unsaved changes") && h.text.contains("Map.ini *"), "{}", h.text);
    assert_eq!(h.app.c5.dirty_names(), vec!["Map.ini".to_string()]);
    assert_eq!(std::fs::read(root.join("Map.ini")).unwrap(), before, "nothing is written before Save");
    // undo and redo
    let mut w = h.app.c5.ini.take().unwrap();
    h.app.ini_act(&mut w, IniAct::Undo);
    h.app.ini_act(&mut w, IniAct::Undo);
    assert!(!w.docs[0].dirty() && w.docs[0].ini.to_bytes() == before);
    h.app.ini_act(&mut w, IniAct::Redo);
    h.app.ini_act(&mut w, IniAct::Redo);
    // save
    h.app.ini_act(&mut w, IniAct::Save);
    assert!(matches!(&w.msg, Some((false, m)) if m.contains("saved and verified")), "{:?}", w.msg);
    let after = std::fs::read(root.join("Map.ini")).unwrap();
    assert_eq!(after, w.docs[0].ini.to_bytes());
    assert_ne!(after, before);
    assert_eq!(std::fs::read(root.join("Map.ini.bak")).unwrap(), before);
    let bk = root.join("data-dir").join("backups").join("server");
    assert!(std::fs::read_dir(&bk).unwrap().flatten().any(|d| std::fs::read_dir(d.path()).unwrap().count() == 1), "a timestamped copy");
    assert!(!w.docs[0].dirty() && root.join("MapWar.ini").is_file());
    // the zone rules: the original server needs the map file before the zone
    h.app.ini_act(&mut w, IniAct::AddZone(11, "F"));
    assert!(matches!(&w.msg, Some((true, m)) if m.contains("11.svmap")), "{:?}", w.msg);
    h.app.ini_act(&mut w, IniAct::AddZone(13, "F"));
    assert!(matches!(&w.msg, Some((true, m)) if m.contains("Only the next id")), "{:?}", w.msg);
    assert!(!w.docs[0].dirty());
    std::fs::copy(maps.join("10.svmap"), maps.join("11.svmap")).unwrap();
    let path = maps.join("11.svmap");
    h.app.folder.as_mut().unwrap().maps.insert(11, FolderMap { path, svmap: None });
    h.app.ini_act(&mut w, IniAct::AddZone(11, "D"));
    assert!(w.docs[0].ini.zone(11).is_some() && w.docs[0].ini.map_count() == Some(12), "{:?}", w.msg);
    // the war file counts war maps: the next id is 101
    h.app.ini_act(&mut w, IniAct::File(1));
    assert_eq!(w.add_id, 101);
    h.app.ini_act(&mut w, IniAct::AddZone(105, "F"));
    assert!(matches!(&w.msg, Some((true, _))), "only the next war id");
    h.app.ini_act(&mut w, IniAct::AddZone(101, "F"));
    assert_eq!(w.docs[1].ini.map_count(), Some(2));
    h.app.ini_act(&mut w, IniAct::File(0));
    h.app.ini_act(&mut w, IniAct::Discard);
    assert!(w.docs.iter().all(|d| !d.dirty()), "discard puts every file back to what is on disk");
    assert_eq!(MapIni::parse(&std::fs::read(root.join("MapWar.ini")).unwrap()).map_count(), Some(1), "MapWar.ini was never saved");
    // pick on map: the rebirth position of zone 9 comes from a click on map 9
    w.zone = Some(9);
    h.app.ini_act(&mut w, IniAct::Pick(9, "RebirthMapPos1"));
    h.app.c5.ini = Some(w);
    assert_eq!(h.app.c5.pick.as_ref().map(|p| (p.zone, p.map)), Some((9, 9)));
    assert!(h.app.pick_banner_text(h.app.active).is_some_and(|t| t.contains("RebirthMapPos1") && t.contains("zone 9")));
    h.app.finish_pick([10.0, 2.0, 20.0]);
    assert!(h.app.c5.pick.is_none());
    let w = h.app.c5.ini.as_ref().unwrap();
    assert_eq!(w.docs[0].own_value(9, "RebirthMapPos1").as_deref(), Some("9, 10.00, 2.00, 20.00"));
    assert!(w.docs[0].dirty(), "the pick is an edit of the file, not written yet");
    // a cancelled pick leaves the file alone
    let mut w = h.app.c5.ini.take().unwrap();
    h.app.ini_act(&mut w, IniAct::Pick(9, "RebirthMapPos2"));
    h.app.c5.ini = Some(w);
    h.app.cancel_picks();
    assert!(h.app.c5.pick.is_none() && h.app.c5.ini.as_ref().unwrap().docs[0].own_value(9, "RebirthMapPos2").is_none());
    h.frames(2);
    assert_eq!(std::fs::read(client.join("world").join("9.wld")).unwrap(), client_before, "the client is never written");
    assert!(!client.join("data").exists());
}

/// The commands are off without a server data folder or a client, with the reason; a Map.ini without MapCount is not opened.
#[test]
fn c5_commands_say_why_they_are_off() {
    let root = TempDir::new("c5-off");
    let (client, maps) = crate::testutil::paired_folders(&root);
    let mut h = Harness::new(None, StartOptions { folder: Some(maps.clone()), maps: vec![9], ..Default::default() });
    wait_for_folder(&mut h);
    let e = h.app.can_run(Cmd::MapIniEditor).unwrap_err();
    assert!(e.contains("Map.ini"), "{e}");
    assert!(h.app.can_run(Cmd::AddMap).unwrap_err().contains("client"));
    h.app.open_ini_editor();
    assert!(h.app.c5.ini.is_none() && h.app.toast.is_some());
    std::fs::write(root.join("Map.ini"), b"[SET_ZONE_1]\r\nMapType = F\r\n").unwrap();
    h.app.open_ini_editor();
    assert!(h.app.c5.ini.is_none() && matches!(&h.app.dialog, Some(Dialog::Error(e)) if e.contains("MapCount")));
    h.app.dialog = None;
    std::fs::write(root.join("Map.ini"), C5_MAP_INI).unwrap();
    assert!(h.app.can_run(Cmd::MapIniEditor).is_ok());
    assert!(h.app.can_run(Cmd::AddMap).unwrap_err().contains("client"), "the client is still missing");
    let ctx = h.ctx.clone();
    h.app.open_client(&client, &ctx);
    assert!(h.app.can_run(Cmd::AddMap).is_ok());
}

/// The Add a map window: the original server gets the next id only; Create writes the staged client files, the server map and
/// the zone, opens the map and leaves the client folder alone; Afterglow takes a gap and a blank field.
#[test]
fn c5_add_a_map_window() {
    let root = TempDir::new("c5-add-window");
    let (mut h, client, maps) = c5_harness(&root);
    let names = ["9.wld", "9.txt", "10.wld"];
    let client_snapshot: Vec<Vec<u8>> = names.iter().map(|n| std::fs::read(client.join("world").join(n)).unwrap()).collect();
    h.size = Vec2::new(1400.0, 1500.0);
    h.app.open_add_map();
    h.frames(3);
    for want in ["Add a map", "the next id", "Copy a client map", "Blank field", "Create writes", "The editor cannot do", "Loading picture 11.bmp", "data/world/Field/11/", "Terrain textures", "Map.ini: [SET_ZONE_11], MapCount 12"] {
        assert!(h.text.contains(want), "{want}: {}", h.text);
    }
    assert_eq!(h.app.c5.add.as_ref().unwrap().id, 11);
    // nothing picked yet: Create refuses
    let mut w = h.app.c5.add.take().unwrap();
    let ctx = h.ctx.clone();
    h.app.add_map_create(&ctx, &mut w);
    assert!(matches!(&w.done, Some(Done { error: Some(e), .. }) if e.contains("Pick the client map")));
    assert!(!root.join("out").exists() && !maps.join("11.svmap").exists());
    w.done = None;
    w.copy_id = Some(9);
    h.app.c5.add = Some(w);
    h.frames(2);
    assert!(h.text.contains("field, size 64") && h.text.contains("3 portals"), "{}", h.text);
    let mut w = h.app.c5.add.take().unwrap();
    h.app.add_map_create(&ctx, &mut w);
    let done = w.done.take().unwrap();
    assert!(done.error.is_none(), "{:?}", done.error);
    // six files plus the light map the copy gets because the source has none
    assert_eq!(done.written.len(), 7, "{:?}", done.written);
    assert!(done.written.iter().any(|(w, _)| w.contains("Field/11/11_00_L.dds")), "{:?}", done.written);
    assert!(done.written.iter().all(|(_, p)| !p.contains(':') && !p.contains('\\')), "relative names only: {:?}", done.written);
    w.done = Some(done);
    h.app.c5.add = Some(w);
    h.frames(3);
    // the files, the tab, the map list and the chips
    let out = root.join("out");
    assert!(out.join("data").join("world").join("11.wld").is_file() && out.join("data").join("interface").join("MiniMap").join("11.tga").is_file());
    assert!(maps.join("11.svmap").is_file());
    assert!(h.app.docs.iter().any(|d| d.map_id == Some(11)) && h.app.folder.as_ref().unwrap().maps.contains_key(&11), "the new map is open");
    let ini = MapIni::parse(&std::fs::read(root.join("Map.ini")).unwrap());
    assert_eq!((ini.map_count(), ini.zone_value(11, "MapType").as_deref()), (Some(12), Some("F")));
    assert!(h.text.contains("Done"), "{}", h.text);
    assert!(h.app.client.as_ref().unwrap().wld_state(11).staged, "the client reads the staged map");
    let now: Vec<Vec<u8>> = names.iter().map(|n| std::fs::read(client.join("world").join(n)).unwrap()).collect();
    assert_eq!(now, client_snapshot, "the client folder is untouched");
    assert!(!client.join("data").exists());
    assert_eq!(h.app.c5.add.as_ref().unwrap().map_ini.as_ref().and_then(MapIni::map_count), Some(12));
    // Afterglow: any free id, a blank field
    h.app.settings.target_server = crate::settings::TargetServer::Afterglow;
    let mut w = h.app.c5.add.take().unwrap();
    w.done = None;
    w.copy = false;
    w.id = 150;
    w.size = 1024;
    w.look = 9;
    h.app.c5.add = Some(w);
    h.frames(3);
    assert!(h.text.contains("100 to 200"), "the item warning: {}", h.text);
    let mut w = h.app.c5.add.take().unwrap();
    h.app.add_map_create(&ctx, &mut w);
    let done = w.done.take().unwrap();
    assert!(done.error.is_none(), "{:?}", done.error);
    let ini = MapIni::parse(&std::fs::read(root.join("Map.ini")).unwrap());
    assert_eq!(ini.map_count(), Some(151));
    assert!(maps.join("150.svmap").is_file());
    // id 200 and an id that is there are refused, nothing is written
    for id in [200u32, 9] {
        w.id = id;
        w.done = None;
        let files = std::fs::read_dir(&maps).unwrap().count();
        h.app.add_map_create(&ctx, &mut w);
        assert!(w.done.as_ref().unwrap().error.is_some(), "id {id}");
        assert_eq!(std::fs::read_dir(&maps).unwrap().count(), files);
    }
    assert!(!out.join("data").join("world").join("200.wld").exists());
}

/// The Share client changes window: it lists the staged files and says the patch is unsigned; Export patch writes `psNNNN.patch`
/// to a folder outside the client; Install and Uninstall work on a test client made in the temp dir; the client folder the editor
/// reads is refused as a target and stays byte-identical.
#[test]
fn c6_share_window() {
    use super::c6_ui::Act;
    let root = TempDir::new("c6-share-window");
    let (mut h, client, _maps) = c5_harness(&root);
    assert!(h.app.can_run(Cmd::ShareClient).is_ok());
    let out = root.join("out");
    std::fs::create_dir_all(out.join("data/world")).unwrap();
    std::fs::write(out.join("data/world/9.wld"), [3u8; 40]).unwrap();
    std::fs::write(out.join("data/world/9.wld.bak"), [1u8; 4]).unwrap();
    let client_files = |dir: &Path| -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
        v.sort();
        v
    };
    let before = client_files(&client);
    h.size = Vec2::new(1400.0, 1500.0);
    h.app.open_share();
    h.frames(3);
    for want in ["Share client changes", "Staged files: 1", "world/9.wld", "unsigned", "Export patch", "Install into a test client", "not proven in the game"] {
        assert!(h.text.contains(want), "{want}: {}", h.text);
    }
    // export: there is no default number, so Export patch needs one typed (this client has no Version.ini and no patch files)
    assert_eq!(h.app.c6.win.as_ref().unwrap().number(), None);
    let dest = root.join("share");
    h.app.share_act(Act::ExportPatch, Some(dest.clone()));
    assert!(h.app.c6.win.as_ref().unwrap().msg.iter().any(|(bad, t)| *bad && t.contains("patch number")), "{:?}", h.app.c6.win.as_ref().unwrap().msg);
    assert!(!dest.exists());
    {
        let w = h.app.c6.win.as_mut().unwrap();
        w.number_text = "109".to_string();
        w.typed = true;
    }
    h.app.share_act(Act::ExportPatch, Some(dest.clone()));
    assert!(dest.join("ps0109.patch").is_file());
    let msg = h.app.c6.win.as_ref().unwrap().msg.clone();
    assert!(msg.iter().all(|(bad, _)| !bad), "{msg:?}");
    assert!(msg.iter().any(|(_, t)| t.contains("unsigned")), "the result says the patch is unsigned: {msg:?}");
    assert!(msg.iter().all(|(_, t)| !t.contains(":\\")), "no drive paths: {msg:?}");
    // install into the client the editor reads: refused
    h.app.share_act(Act::Pick, Some(client.clone()));
    h.app.share_act(Act::Install, None);
    let msg = h.app.c6.win.as_ref().unwrap().msg.clone();
    assert!(msg.iter().any(|(bad, t)| *bad && t.contains("separate test copy")), "{msg:?}");
    assert_eq!(client_files(&client), before, "the client the editor reads is untouched");
    // a test client in the temp dir
    let test = root.join("test-client");
    crate::share::tests_support::tiny_client(&test);
    let sah0 = std::fs::read(test.join("data.sah")).unwrap();
    h.app.share_act(Act::Pick, Some(test.clone()));
    h.frames(2);
    assert!(h.text.contains("Nothing installed by this editor yet"), "{}", h.text);
    h.app.share_act(Act::Install, None);
    let msg = h.app.c6.win.as_ref().unwrap().msg.clone();
    assert!(msg.iter().all(|(bad, _)| !bad) && msg.iter().any(|(_, t)| t.contains("Installed 1 new")), "{msg:?}");
    h.frames(2);
    assert!(h.text.contains("Installed by this editor: 1 install"), "{}", h.text);
    assert_ne!(std::fs::read(test.join("data.sah")).unwrap(), sah0);
    h.app.share_act(Act::Uninstall, None);
    assert_eq!(std::fs::read(test.join("data.sah")).unwrap(), sah0);
    assert!(h.app.c6.win.as_ref().unwrap().msg.iter().all(|(bad, _)| !bad));
    assert_eq!(client_files(&client), before);
}

/// The patch number field: no default, both candidates shown, the higher one suggested, and a typed number kept.
#[test]
fn c6_patch_number_field() {
    use super::c6_ui::Act;
    let root = TempDir::new("c6-number");
    let (mut h, client, _maps) = c5_harness(&root);
    let out = root.join("out");
    std::fs::create_dir_all(out.join("data/world")).unwrap();
    std::fs::write(out.join("data/world/9.wld"), [3u8; 40]).unwrap();
    h.size = Vec2::new(1400.0, 1500.0);
    h.app.open_share();
    h.frames(3);
    // neither candidate exists
    assert_eq!(h.app.c6.win.as_ref().unwrap().number_text, "");
    for want in ["Version.ini CurrentVersion: none found", "Highest patch file: none found", "Suggested: none"] {
        assert!(h.text.contains(want), "{want}: {}", h.text);
    }
    // Version.ini only: 108 -> 109
    std::fs::write(client.join("Version.ini"), "[Version]\r\nCheckVersion=108\r\nCurrentVersion=108\r\n").unwrap();
    h.app.share_act(Act::Refresh, None);
    h.frames(3);
    assert_eq!(h.app.c6.win.as_ref().unwrap().number_text, "109");
    for want in ["Version.ini CurrentVersion 108: next 109", "Highest patch file: none found", "Suggested: 109"] {
        assert!(h.text.contains(want), "{want}: {}", h.text);
    }
    // an export folder with ps0110.patch: both candidates, the higher one (111) is suggested
    let dest = root.join("share");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("ps0110.patch"), b"x").unwrap();
    h.app.c6.last_export = Some(dest.clone());
    h.app.share_act(Act::Refresh, None);
    h.frames(3);
    assert_eq!(h.app.c6.win.as_ref().unwrap().number_text, "111");
    for want in ["Version.ini CurrentVersion 108: next 109", "Highest patch file ps0110.patch in share: next 111", "Suggested: 111 (the higher)"] {
        assert!(h.text.contains(want), "{want}: {}", h.text);
    }
    // exporting with the suggested number writes ps0111 and the field moves on to 112
    h.app.share_act(Act::ExportPatch, Some(dest.clone()));
    assert!(dest.join("ps0111.patch").is_file());
    assert_eq!(h.app.c6.win.as_ref().unwrap().number_text, "112");
    // a typed number is kept when the suggestion changes
    {
        let w = h.app.c6.win.as_mut().unwrap();
        w.number_text = "150".to_string();
        w.typed = true;
    }
    std::fs::write(dest.join("ps0200.patch"), b"x").unwrap();
    h.app.share_act(Act::Refresh, None);
    let w = h.app.c6.win.as_ref().unwrap();
    assert_eq!((w.number_text.as_str(), w.hint.suggested), ("150", Some(201)));
    // an existing patch file is still refused
    h.app.c6.win.as_mut().unwrap().number_text = "111".to_string();
    h.app.share_act(Act::ExportPatch, Some(dest.clone()));
    assert!(h.app.c6.win.as_ref().unwrap().msg.iter().any(|(bad, t)| *bad && t.contains("already")), "{:?}", h.app.c6.win.as_ref().unwrap().msg);
}

/// The real pick-and-export path: Version.ini says 108 and the folder picked for the export already holds ps0115.patch, so the
/// patch written is ps0116 (the suggestion is recomputed with the picked folder before the export). The hint names the file as it is
/// on disk. A typed number that is taken is refused with a message and nothing is replaced.
#[test]
fn c6_export_recomputes_the_number_with_the_picked_folder() {
    use super::c6_ui::Act;
    let root = TempDir::new("c6-number-pick");
    let (mut h, client, _maps) = c5_harness(&root);
    std::fs::write(client.join("Version.ini"), "[Version]\r\nCurrentVersion=108\r\n").unwrap();
    let out = root.join("out");
    std::fs::create_dir_all(out.join("data/world")).unwrap();
    std::fs::write(out.join("data/world/9.wld"), [3u8; 40]).unwrap();
    h.size = Vec2::new(1400.0, 1500.0);
    h.app.open_share();
    h.frames(3);
    assert_eq!(h.app.c6.win.as_ref().unwrap().number_text, "109", "before a folder is picked the field shows the Version.ini candidate");
    let dest = root.join("picked");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("PS0115.PATCH"), b"x").unwrap();
    h.app.share_act(Act::ExportPatch, Some(dest.clone()));
    let msg = h.app.c6.win.as_ref().unwrap().msg.clone();
    assert!(msg.iter().all(|(bad, _)| !bad), "{msg:?}");
    assert!(dest.join("ps0116.patch").is_file(), "{:?}", std::fs::read_dir(&dest).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>());
    assert!(!dest.join("ps0109.patch").exists());
    // the highest file is named as found
    std::fs::remove_file(dest.join("ps0116.patch")).unwrap();
    h.app.share_act(Act::Refresh, None);
    h.frames(3);
    assert!(h.text.contains("Highest patch file PS0115.PATCH in picked: next 116"), "{}", h.text);
    // a typed number that is taken is refused, with the file untouched (same name in lower case, so this holds on any file system)
    std::fs::write(dest.join("ps0115.patch"), b"x").unwrap();
    {
        let w = h.app.c6.win.as_mut().unwrap();
        w.number_text = "115".to_string();
        w.typed = true;
    }
    h.app.share_act(Act::ExportPatch, Some(dest.clone()));
    let msg = h.app.c6.win.as_ref().unwrap().msg.clone();
    assert!(msg.iter().any(|(bad, t)| *bad && t.contains("already in that folder")), "{msg:?}");
    assert_eq!(std::fs::read(dest.join("ps0115.patch")).unwrap(), b"x");
}

/// The client chip carries the episode notes: a client without `NpcQuest.SData` names it in the chip text, a complete one shows no
/// note and no chip.
#[test]
fn the_client_chip_shows_the_episode_notes() {
    use shaiya_svmap_tools::episode::client_notes;
    let root = TempDir::new("episode-chip");
    let client = client_dir(&root);
    std::fs::write(client.join("world").join("1.wld"), crate::testutil::tiny_wld(4).to_bytes()).unwrap();
    // a sysmsg with every id of the map-name table, so only the missing SData tables are noted
    let mut sys = String::new();
    for id in shaiya_svmap_tools::episode::map_name_ids() {
        sys.push_str(&format!("{id}\t\t\"name {id}\"\r\n"));
    }
    std::fs::write(client.join("sysmsg-uni.txt"), crate::testutil::utf16_bom(&sys)).unwrap();
    let mut h = Harness::new(None, StartOptions::default());
    let ctx = h.ctx.clone();
    h.app.open_client(&client, &ctx);
    assert!(h.app.client.is_some());
    let (label, hover) = h.app.client_chip_text();
    assert!(label.starts_with("client: "), "{label}");
    assert!(hover.contains("NpcQuest.SData is missing"), "{hover}");
    assert!(hover.contains("Monster.SData is missing"), "{hover}");
    h.frames(2);
    assert!(h.text.contains(&label), "the chip is drawn: {}", h.text);

    // a complete client (both tables read): no note, nothing in the chip
    let mut rules = ClientRules::open(&client).unwrap();
    rules.monsters = Some(shaiya_clientdata::Monsters { records: Vec::new() });
    rules.npcs = Some(shaiya_clientdata::NpcQuest { lists: Vec::new(), quests_offset: 0 });
    rules.problems.clear();
    assert!(client_notes(&rules).is_empty());
    h.app.adopt_client(&client, rules);
    assert!(h.app.client_notes.is_empty());
    let (_, hover) = h.app.client_chip_text();
    assert!(!hover.contains("differs from US EP4.5"), "{hover}");
    h.frames(2);
    assert!(!h.text.contains(&label), "no chip for a complete client: {}", h.text);
}

/// With no data folder (`no_data_folder`), the status bar says settings and backups are off, and nothing is persisted.
#[test]
fn no_data_folder_shows_the_status_bar_warning() {
    let mut h = Harness::new(None, StartOptions { no_data_folder: true, ..Default::default() });
    h.app.settings.first_run_done = true;
    h.frames(3);
    assert!(h.text.contains("No data folder: settings and backups are off; use --data-dir"), "{}", h.text);
    h.app.flush_settings();
    assert!(!h.app.persist && h.app.settings_path.is_none() && h.app.data_dir.is_none());
    let mut h = Harness::new(None, StartOptions::default());
    h.app.settings.first_run_done = true;
    h.frames(3);
    assert!(!h.text.contains("No data folder"), "{}", h.text);
}
