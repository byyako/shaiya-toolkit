//! Headless test harness (feature `testing`): a [`ShellApp`] in an `egui::Context` with synthetic input, the text of the last frame
//! captured. Generalised from the svmap editor's `app_tests.rs` `Harness`. Native file dialogs are off; no CJK font is loaded.

use crate::{ShellApp, ShellOptions, ToolApp};
use egui::{Event, Modifiers, Pos2, RawInput, Rect, Vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A file the harness "drops" on the window.
#[derive(Debug)]
struct TestFile(PathBuf);

impl egui::DroppedFile for TestFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

pub struct Harness<A: ToolApp> {
    pub ctx: egui::Context,
    pub app: ShellApp<A>,
    t: f64,
    text: String,
    copied: Vec<String>,
    opened_urls: Vec<String>,
    /// The window size of the next frames.
    pub size: Vec2,
}

/// Every text shape of a frame, one per line.
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

impl<A: ToolApp> Harness<A> {
    /// A shell with the given options (`load_fonts` is forced off). Use `ShellOptions { no_settings: true, .. }` or a `data_dir` of a
    /// temporary folder.
    pub fn new(mut opts: ShellOptions) -> Harness<A> {
        opts.load_fonts = false;
        opts.smoke = None;
        let ctx = egui::Context::default();
        let mut app = ShellApp::<A>::new(ctx.clone(), &opts, None);
        app.shell.set_dialogs_enabled(false);
        app.shot_headless();
        Harness { ctx, app, t: 0.0, text: String::new(), copied: Vec::new(), opened_urls: Vec::new(), size: Vec2::new(1280.0, 800.0) }
    }

    /// No settings, no files.
    pub fn bare() -> Harness<A> {
        Harness::new(ShellOptions { no_settings: true, ..Default::default() })
    }

    fn run(&mut self, input_events: Vec<Event>, dropped: Vec<PathBuf>) {
        self.t += 0.05;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.t),
            events: input_events,
            dropped_files: dropped.into_iter().map(|p| std::sync::Arc::new(TestFile(p)) as egui::DroppedFileHandle).collect(),
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| app.frame(ui));
        out.textures_delta.clear();
        self.text = all_text(&out.shapes);
        self.copied = out.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t.clone()) } else { None }).collect();
        self.opened_urls = out.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::OpenUrl(u) = c { Some(u.url.clone()) } else { None }).collect();
    }

    /// One frame with these events.
    pub fn frame(&mut self, events: Vec<Event>) {
        self.run(events, Vec::new());
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(Vec::new());
        }
    }

    /// Press and release a key.
    pub fn key(&mut self, key: egui::Key, modifiers: Modifiers) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
    }

    /// Move the pointer to `pos`, then press and release the primary button there (three frames).
    pub fn click(&mut self, pos: Pos2) {
        let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
    }

    pub fn type_text(&mut self, text: &str) {
        self.frame(vec![Event::Text(text.to_string())]);
    }

    /// Drop files on the window (one frame, as the window system delivers them).
    pub fn drop_files(&mut self, paths: &[PathBuf]) {
        self.run(Vec::new(), paths.to_vec());
    }

    /// The text of the last frame (every text shape, one per line).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Everything the app put on the clipboard in the last frame.
    pub fn copied(&self) -> &[String] {
        &self.copied
    }

    /// Every URL the app asked the browser to open in the last frame.
    pub fn opened_urls(&self) -> &[String] {
        &self.opened_urls
    }

    /// For a harness built with `ShellOptions { shot: Some(..), .. }`: runs frames until the scene is set up and the background work is idle (the
    /// moment the real `--shot` would take its screenshot). `Err` is the scene's failure or a timeout.
    pub fn wait_scene(&mut self, timeout: Duration) -> Result<(), String> {
        self.wait_until(timeout, |h| h.app.shot_result().is_some());
        match self.app.shot_result() {
            Some(r) => r,
            None => Err("the scene did not settle in time".to_string()),
        }
    }

    /// Runs frames (sleeping a little between them) until `done` returns true or the timeout passes. Returns whether it was reached.
    pub fn wait_until(&mut self, timeout: Duration, mut done: impl FnMut(&mut Self) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            self.frame(Vec::new());
            if done(self) {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
