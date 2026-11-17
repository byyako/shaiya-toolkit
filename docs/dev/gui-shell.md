# The GUI shell (`shaiya-ui`)

Every toolkit GUI is a `ToolApp` run by `shaiya_ui::run`. The shell gives it the window, the menus, settings, the Activity panel, background tasks, dialogs, undo and the test harness, so a tool is only its central panel and its logic.

## Writing a tool

```rust
use shaiya_ui::{AppInfo, Cx, ToolApp};

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Settings { /* the tool's own settings */ }

struct MyApp { /* state */ }

impl ToolApp for MyApp {
    type Settings = Settings;
    fn info() -> AppInfo { AppInfo { id: "shaiya-my-tool", name: "Shaiya My Tool", /* version, git, target, icon, filters, help */ } }
    fn new(cx: &mut Cx<'_, Settings>) -> Self { MyApp { } }
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_, Settings>) { /* the central panel */ }
    fn open(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Cx<'_, Settings>) { /* dialog, drop, command line, recent */ }
}

fn main() -> std::process::ExitCode { shaiya_ui::run::<MyApp>() }
```

Optional methods: `menu` (extra items in File, Edit, View, Tools, Help), `commands` and `run_command` (the command palette and shortcuts), `undo` / `redo`, `unsaved` and `save_all` (the quit dialog), `prefs` (the tool's section of Preferences), and `has_documents`, `save`, `save_as`, `close_document` (then File shows Save, Save as... and Close).

## What `Cx` gives you

`cx.shell` is the `Shell`; `cx.settings` is your settings, stored under `tool` in `<tool-id>/settings.json`.

| Need | Call |
|---|---|
| Background work | `cx.shell.tasks().spawn(TaskSpec { title, cancellable, exclusive }, \|job\| { ... })` returns a `TaskHandle`; call `poll()` every frame. Panics become `TaskOutcome::Panicked`; tasks with the same `exclusive` key run one at a time. |
| Status text | `cx.shell.status("...")` (6 seconds) |
| An error | `cx.shell.error("Cannot open", &err)`: the banner, the log and the Activity list; Details and Copy give a bug-report text |
| A question | `cx.shell.confirm("id", &Confirm { title, text, buttons: ["Overwrite", "Cancel"], danger })`: call it every frame; it returns `Some(button)` once answered. Buttons are verbs, never "OK". |
| File dialogs | `pick_file`, `pick_files`, `pick_folder`, `save_file` (they remember the last folder per purpose); "Open path..." and drag and drop always work without a portal |
| Recent files | `cx.shell.add_recent(path)` |
| Empty state | `cx.shell.empty_state(ui, "hint")`: hint, the "your own copy" statement, recent files |
| Korean or Chinese text | `cx.shell.need_cjk_font()` when a decoded name has such characters and no font is loaded |
| Colours | `cx.shell.pal()`: `text`, `weak`, `link`, `ok`, `error`, `warning`, `info`, ... (never write colour literals) |
| Autosave and backups | `cx.shell.recovery()` and `cx.shell.backups()` (`None` with `--no-settings`) |
| The "my client folders" list | `cx.shell.shared().clients` |
| Another toolkit program | `cx.shell.launch_sibling("shaiya-sdata-editor", &[path.into()])` |
| Show a folder or open a web page | `ui.ctx().open_url(egui::OpenUrl::new_tab(url))`, only from a click; the operating system opens it. Never open anything on start-up or in the background |

## Undo

`shaiya_ui::undo::History<E>` is generic over an edit type implementing `Edit<M>` (`apply` and `invert`): steps with the same merge key inside 1500 ms merge, at most 5000 steps, `mark_saved` and `is_dirty` track the saved state.

## Settings and the data folder

Root: `SHAIYA_TOOLKIT_HOME`, else `portable.txt` next to the program (`shaiya-toolkit-data`), else the user's configuration folder. `shared.json` (theme, scale, font, client folders) is shared by all tools and merged before each write because several tools may run at once; `<tool-id>/settings.json` holds `shell` (window size, recent files with pins, last folders, panel state, autosave and backup counts) and `tool`. Every key is optional, values are clamped on load, unknown keys are kept. `--data-dir DIR` and `--no-settings` apply to one run.

## Flags every GUI has

`[FILES...]`, `--data-dir DIR`, `--no-settings`, `--smoke [REPORT_FILE]`.

## Reserved shortcuts

Ctrl is Cmd on macOS. Ctrl+O open, Ctrl+Shift+O open path, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+W close, Ctrl+Q quit, Ctrl+Z undo, Ctrl+Y and Ctrl+Shift+Z redo, Ctrl+, preferences, Ctrl+= / Ctrl+- / Ctrl+0 zoom, Ctrl+J activity panel, Ctrl+Shift+P command palette, F1 help. Tools use others.

## Tests

Enable the `testing` feature of `shaiya-ui` in your dev-dependencies and use `shaiya_ui::testing::Harness`: a headless `egui::Context` with synthetic input and the text of the last frame.

```rust
let mut h = Harness::<MyApp>::bare();      // no settings, no dialogs, no fonts
h.frames(2);
assert!(h.text().contains("Drop files here"));
h.drop_files(&[path]);
h.key(egui::Key::P, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT);
h.wait_until(Duration::from_secs(5), |h| /* a task finished */ true);
```

Run the real window with `--smoke report.txt` to check the graphics stack. Left out on purpose: network access of any kind, multiple windows per process, docking, plugins, custom themes, translations, screen-reader support, window position memory.
