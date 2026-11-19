//! Harness tests of the shell itself, with a tiny dummy tool.

use crate::testing::Harness;
use crate::{AppInfo, Confirm, Cx, Menu, ShellOptions, ToolApp};
use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
struct DummySettings {
    counter: u32,
}

#[derive(Default)]
struct Dummy {
    opened: Vec<PathBuf>,
    menu_calls: u32,
    ask: Option<Confirm>,
    answer: Option<usize>,
}

impl ToolApp for Dummy {
    type Settings = DummySettings;
    fn info() -> AppInfo {
        AppInfo {
            id: "dummy-tool",
            name: "Dummy Tool",
            version: "9.9.9",
            git: "abc123",
            target: "test-target",
            icon_rgba: &[],
            icon_size: 0,
            open_filters: &[("All files", &["*"])],
            help: "Dummy help text for the help window.",
        }
    }
    fn new(cx: &mut Cx<'_, DummySettings>) -> Dummy {
        cx.settings.counter += 1;
        Dummy::default()
    }
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_, DummySettings>) {
        if let Some(c) = &self.ask {
            if let Some(a) = cx.shell.confirm("ow", c) {
                self.answer = Some(a);
                self.ask = None;
            }
        }
        if self.opened.is_empty() {
            cx.shell.empty_state(ui, "Drop files here or press Ctrl+O");
        } else {
            for p in &self.opened {
                ui.label(p.display().to_string());
            }
        }
    }
    fn open(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_, DummySettings>) {
        for p in &paths {
            cx.shell.add_recent(p);
        }
        self.opened.extend(paths);
    }
    fn menu(&mut self, _menu: Menu, _ui: &mut egui::Ui, _cx: &mut Cx<'_, DummySettings>) {
        self.menu_calls += 1;
    }
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("shaiya-ui-harness-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_empty_state_says_what_to_do_and_the_own_copy_statement() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    let t = h.text().to_string();
    assert!(t.contains("Drop files here or press Ctrl+O"), "{t}");
    assert!(t.contains("These tools work on your own copy of the game"), "{t}");
    for menu in ["File", "Edit", "View", "Tools", "Help"] {
        assert!(t.contains(menu), "menu {menu} missing: {t}");
    }
    assert!(t.contains("v9.9.9"), "the version sits in the status bar: {t}");
    assert!(t.contains("Welcome"), "the first run welcome: {t}");
}

#[test]
fn dropping_files_opens_them_through_the_tool() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(1);
    let (a, b) = (PathBuf::from("Z:/x/data.sah"), PathBuf::from("Z:/x/game.exe"));
    h.drop_files(&[a.clone(), b.clone()]);
    h.frames(2);
    assert_eq!(h.app.app.opened, vec![a, b]);
    assert!(h.text().contains("data.sah") && h.text().contains("game.exe"), "{}", h.text());
    assert_eq!(h.app.shell.recent_paths().len(), 2);
}

#[test]
fn the_theme_switch_changes_the_palette() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    let dark = h.app.shell.pal();
    h.app.shell.shared_mut().theme = crate::theme::Theme::Light;
    h.frames(3);
    let light = h.app.shell.pal();
    assert_ne!(dark, light);
    assert!(light.text.r() < 128 && dark.text.r() > 128, "light theme has dark text");
}

#[test]
fn ctrl_shift_p_opens_the_palette_and_escape_closes_it() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    h.key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    assert!(h.text().contains("Command palette"), "{}", h.text());
    assert!(h.text().contains("Preferences..."), "the shell commands are listed");
    h.type_text("zoo");
    h.frames(1);
    assert!(h.text().contains("Zoom in") && !h.text().contains("Preferences..."), "{}", h.text());
    h.key(Key::Escape, Modifiers::NONE);
    h.frames(2);
    assert!(!h.text().contains("Type a command"), "the palette closed");
}

#[test]
fn a_click_outside_the_palette_closes_it() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    h.key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    assert!(h.text().contains("Type a command"), "{}", h.text());
    h.click(egui::pos2(10.0, 700.0));
    h.frames(2);
    assert!(!h.text().contains("Type a command"), "the palette closed: {}", h.text());
}

#[test]
fn a_click_inside_the_palette_keeps_it_open() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    h.key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    h.click(egui::pos2(640.0, 90.0));
    h.frames(2);
    assert!(h.text().contains("Command palette"), "{}", h.text());
    assert!(h.text().contains("Preferences..."), "the rows are still listed: {}", h.text());
}

#[test]
fn help_activity_and_preferences_open_from_the_keyboard() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    h.key(Key::F1, Modifiers::NONE);
    h.frames(2);
    assert!(h.text().contains("Dummy help text"), "{}", h.text());
    assert!(!h.app.shell.activity_open());
    h.key(Key::J, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.app.shell.activity_open());
    assert!(h.text().contains("Activity"), "{}", h.text());
    h.key(Key::Comma, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.text().contains("Preferences") && h.text().contains("UI scale"), "{}", h.text());
}

#[test]
fn settings_written_with_a_data_dir_load_back_and_keep_unknown_keys() {
    let d = temp("settings");
    // a settings file from a "newer build": an unknown key at the top and inside the shell part
    let tool_dir = d.join("dummy-tool");
    std::fs::create_dir_all(&tool_dir).unwrap();
    std::fs::write(tool_dir.join("settings.json"), r#"{"format": 1, "future": {"x": 1}, "shell": {"newer_shell_key": 7}, "tool": {"counter": 5}}"#).unwrap();
    let opts = ShellOptions { data_dir: Some(d.clone()), ..Default::default() };
    {
        let mut h = Harness::<Dummy>::new(opts.clone());
        assert_eq!(h.app.tool_settings.counter, 6, "Dummy::new bumped the counter loaded from the file");
        h.frames(2);
        h.app.shell.add_recent(std::path::Path::new("Z:/x/data.sah"));
        h.app.shell.set_window_size([1000.0, 650.0]);
        h.app.shell.shared_mut().ui_scale = 1.25;
        h.app.flush_settings(true);
    }
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(tool_dir.join("settings.json")).unwrap()).unwrap();
    assert_eq!(raw["future"]["x"], 1, "unknown top-level keys survive");
    assert_eq!(raw["shell"]["newer_shell_key"], 7, "unknown shell keys survive");
    assert_eq!(raw["shell"]["recent"][0]["path"], "Z:/x/data.sah");
    assert_eq!(raw["tool"]["counter"], 6);
    assert!(d.join("shared.json").is_file());
    // a second run loads them back
    let h = Harness::<Dummy>::new(opts);
    assert_eq!(h.app.shell.window_size(), [1000.0, 650.0]);
    assert_eq!(h.app.shell.recent_paths()[0], "Z:/x/data.sah");
    assert_eq!(h.app.shell.shared().ui_scale, 1.25);
    assert!(tool_dir.join("log.txt").is_file(), "the log starts with a data folder");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_settings_writes_nothing() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    h.app.shell.add_recent(std::path::Path::new("x"));
    h.app.flush_settings(true);
    assert!(h.app.shell.data_dir().is_none());
    assert!(h.app.shell.backups().is_none() && h.app.shell.recovery().is_none());
}

#[test]
fn errors_show_in_a_banner_with_details_and_confirmations_wait_for_an_answer() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    let e = shaiya_core::Error::format("Item.SData", Some(40), "SEED checksum failed");
    h.app.shell.error("Cannot open", &e);
    h.frames(2);
    let t = h.text().to_string();
    assert!(t.contains("Cannot open") && t.contains("SEED checksum failed"), "{t}");
    assert!(t.contains("Dismiss") && t.contains("Copy") && t.contains("Details"), "{t}");
    assert!(h.app.shell.activity().iter().any(|l| l.text.contains("Cannot open")), "also in the Activity list");
    let details = h.app.shell.banner_details().unwrap();
    assert!(details.contains("offset: 40") && details.contains("file: Item.SData") && details.contains("Dummy Tool 9.9.9"), "{details}");
    // confirm: shown until answered
    h.app.app.ask = Some(Confirm { title: "Overwrite file?".into(), text: "It exists.".into(), buttons: vec!["Overwrite".into(), "Cancel".into()], danger: Some(0) });
    h.frames(3);
    assert!(h.text().contains("Overwrite file?") && h.text().contains("Overwrite") && !h.text().contains("OK"), "{}", h.text());
    assert_eq!(h.app.app.answer, None);
    h.app.shell.inject_answer("ow", 1);
    h.frames(2);
    assert_eq!(h.app.app.answer, Some(1));
    assert!(h.app.app.ask.is_none());
    h.frames(2);
    assert!(!h.text().contains("Overwrite file?"), "the modal is gone once answered");
}

#[test]
fn quit_with_running_tasks_asks_first() {
    let mut h = Harness::<Dummy>::bare();
    h.frames(2);
    let mut handle = h.app.shell.tasks().spawn(crate::TaskSpec::new("long"), |job| -> shaiya_core::Result<()> {
        while job.check().is_ok() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(shaiya_core::Error::Cancelled)
    });
    h.key(Key::Q, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.text().contains("Cancel 1 task and quit") && h.text().contains("Keep working"), "{}", h.text());
    handle.cancel();
    assert!(h.wait_until(Duration::from_secs(3), |_| handle.poll().is_some()));
}

#[test]
fn siblings_that_are_not_installed_are_not_found() {
    assert_eq!(crate::sibling_path("shaiya-surely-not-installed"), None);
}

#[test]
fn gui_programs_are_the_shipped_ones() {
    assert_eq!(crate::GUI_PROGRAMS, &[("shaiya-toolkit", "Shaiya Toolkit"), ("shaiya-svmap-editor", "Shaiya SVMap Editor")]);
}

#[test]
fn a_sibling_next_to_the_program_is_found() {
    use crate::settings::Os;
    let dir = std::env::temp_dir().join(format!("shaiya-ui-sibling-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let name = if cfg!(windows) { "shaiya-svmap-editor.exe" } else { "shaiya-svmap-editor" };
    assert_eq!(crate::shell::sibling_in(&dir, "shaiya-svmap-editor", Os::CURRENT), None);
    std::fs::write(dir.join(name), b"x").unwrap();
    assert_eq!(crate::shell::sibling_in(&dir, "shaiya-svmap-editor", Os::CURRENT), Some(dir.join(name)));
    // macOS: the sibling bundle next to the running one
    let bundle = dir.join("shaiya-svmap-editor.app").join("Contents").join("MacOS");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("shaiya-svmap-editor"), b"x").unwrap();
    let running = dir.join("shaiya-toolkit.app").join("Contents").join("MacOS");
    assert_eq!(crate::shell::sibling_in(&running, "shaiya-svmap-editor", Os::Mac), Some(bundle.join("shaiya-svmap-editor")));
    let _ = std::fs::remove_dir_all(&dir);
}
