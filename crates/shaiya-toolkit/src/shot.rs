//! The screenshot scenes of `shaiya-toolkit --shot OUT.png --scene NAME DEMO_DIR`. Every scene works on a synthetic demo client (made by
//! `cargo xtask demo-client DIR`, recognised by its marker file) and shows it by name: under `--shot` the shell never prints a saved
//! folder's path, and no scene opens a view that does.
//!
//! A scene is set up once (`begin_scene`), then the shell waits until the background work is idle and asks for the next step
//! (`scene_step`) until there is none; then the window is photographed.

use crate::clients::same_folder;
use crate::sdata_table::Input;
use crate::{Tab, ToolkitApp, ToolkitCx};
use shaiya_archive::edit::{self, EditPlan, InPlaceOptions, Put, PutMode, Source};
use shaiya_archive::journal::{self, JournalMode};
use shaiya_core::Job;
use std::path::{Path, PathBuf};

/// The scenes, in the order the documentation lists them.
pub const SCENES: &[&str] = &["start", "archive", "patch-builder", "apply-history", "sdata-table", "health", "map-preview", "model-preview", "anim-preview", "inspect"];

/// The name the demo client is saved under (the only name a screenshot shows for its folder).
pub const CLIENT_NAME: &str = "Demo client";

/// The loose file that marks a demo client (written by `shaiya_testkit::demo_client`).
pub const MARKER: &str = "shaiya-demo-client.txt";

/// The pack time the queued demo files get (the same as the demo archive's own files; see `shaiya_testkit::demo::DEMO_PACKED_AT`).
const PACKED_AT: i32 = 1_262_304_000;

/// The key the `anim-preview` scene pauses the player at.
const ANIM_SHOT_KEY: f32 = 30.0;

/// Where a scene is.
pub struct SceneRun {
    name: &'static str,
    dir: PathBuf,
    step: u32,
}

fn make_change(dir: &Path) -> Result<(), String> {
    // a real change through the same engine the Archive tab uses, so the history has a real journal
    let plan = EditPlan {
        label: "Demo: add a changelog".into(),
        puts: vec![Put { path: "Demo/changelog.txt".into(), source: Source::Bytes(b"Demo change\r\n".to_vec()), version: 1, mode: PutMode::AddOrReplace }],
        ..EditPlan::default()
    };
    let opts = InPlaceOptions { journal: JournalMode::In(dir.join(journal::DEFAULT_DIR)), legacy_bak: false, check_in_use: false };
    edit::apply_in_place(dir, &plan, &opts, &Job::silent()).map_err(|e| format!("could not make the demo change: {e}"))?;
    // the journal is stamped with the real time; a picture must not show when it was taken, so the demo folder's own journal gets a fixed time
    let root = dir.join(journal::DEFAULT_DIR);
    let list = journal::list(dir, &root).map_err(|e| format!("could not read the demo journal: {e}"))?;
    for (mut j, _) in list {
        j.created_utc = DEMO_TIME.into();
        j.write().map_err(|e| format!("could not date the demo journal: {e}"))?;
    }
    Ok(())
}

/// The time the demo change shows in the history (the same day as the demo archive's own files; no real time reaches a picture).
const DEMO_TIME: &str = "2010-01-01T00:00:00Z";

impl ToolkitApp {
    /// Sets up a scene: saves the demo client (given as the first file of the command line) under [`CLIENT_NAME`] and opens what the scene
    /// needs. `Err` is the reason the scene cannot be shown.
    pub(crate) fn begin_scene(&mut self, name: &str, cx: &mut ToolkitCx<'_>) -> Result<(), String> {
        let Some(scene) = SCENES.iter().copied().find(|s| *s == name) else {
            return Err(format!("unknown scene; the scenes are {}", SCENES.join(", ")));
        };
        let dir = cx.shell.shot_inputs().first().cloned().ok_or("name the demo client folder before the options (cargo xtask demo-client DIR makes one)")?;
        if !dir.join(MARKER).is_file() {
            return Err(format!("that folder is not a demo client (it has no {MARKER}); make one with cargo xtask demo-client DIR"));
        }
        let saved = cx.shell.shared().clients.iter().find(|c| c.name == CLIENT_NAME).map(|c| PathBuf::from(&c.path));
        if let Some(path) = &saved {
            // a saved client of that name (a --data-dir run) must be this very folder: a scene never opens or changes any other
            if !same_folder(path, &dir) {
                return Err(format!("the saved client {CLIENT_NAME:?} is not the demo folder given; use a --data-dir without it, or none"));
            }
        } else {
            self.add_client(CLIENT_NAME, &dir, false, cx).map_err(|e| e.to_string())?;
        }
        match scene {
            "start" => {}
            "inspect" => {
                self.set_tab(Tab::Inspect, cx);
                self.inspect.open(vec![dir.join("data.sah"), dir.join("Version.ini"), dir.join("Demo").join("readme.txt"), dir.join("Demo").join("new-file.txt")], cx);
            }
            "apply-history" => {
                make_change(&dir)?;
                self.select_client(CLIENT_NAME, cx);
            }
            _ => self.select_client(CLIENT_NAME, cx),
        }
        self.scene = Some(SceneRun { name: scene, dir, step: 0 });
        Ok(())
    }

    /// The next step of the scene, taken when the background work is idle. True when a step was taken (the shell waits for idle again),
    /// false when the scene is ready to be photographed.
    pub(crate) fn scene_step(&mut self, cx: &mut ToolkitCx<'_>) -> bool {
        let Some(run) = &mut self.scene else { return false };
        run.step += 1;
        let (name, step, dir) = (run.name, run.step, run.dir.clone());
        let needs_client = !matches!(name, "start" | "inspect");
        if needs_client && !self.archive.is_open() {
            eprintln!("warning: the demo client did not open; the screenshot shows the empty tab");
            return false;
        }
        match (name, step) {
            ("archive", 1) => self.archive.select_path("Interface/demo.tga", cx),
            ("patch-builder", 1) => {
                let source = |name: &str| dir.join("Demo").join(name);
                self.archive.queue_puts(vec![("Demo/readme.txt".into(), source("readme.txt"), PACKED_AT), ("Demo/new-file.txt".into(), source("new-file.txt"), PACKED_AT)]);
                self.patch.from_pending(self.archive.pending_build_input());
                self.set_tab(Tab::Patch, cx);
            }
            ("apply-history", 1) => self.set_tab(Tab::Apply, cx),
            ("sdata-table", 1) => self.archive.select_path("Item/Item.SData", cx),
            ("sdata-table", 2) => {
                let Some(bytes) = self.archive.archive_handle().and_then(|a| a.read("Item/Item.SData").ok()) else {
                    eprintln!("warning: Item/Item.SData could not be read from the demo client");
                    return false;
                };
                self.open_table("Item/Item.SData".to_string(), Input::Bytes(bytes), cx);
            }
            ("health", 1) => self.open_health(Some(CLIENT_NAME), cx),
            ("map-preview", 1) => self.archive.select_path("world/7.wld", cx),
            ("model-preview", 1) => self.archive.select_path("Entity/Building/demo-box.smod", cx),
            ("anim-preview", 1) => self.archive.select_path("Demo/ANI/demo_wave.ANI", cx),
            // the preview is ready now: pause the player at key 30 (Bones is on by default)
            ("anim-preview", 2) => {
                if !self.archive.pause_anim_at(ANIM_SHOT_KEY) {
                    eprintln!("warning: the demo animation did not open as an animation; the screenshot shows what is there");
                    return false;
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_names_are_the_documented_ones() {
        assert_eq!(SCENES, &["start", "archive", "patch-builder", "apply-history", "sdata-table", "health", "map-preview", "model-preview", "anim-preview", "inspect"]);
    }
}
