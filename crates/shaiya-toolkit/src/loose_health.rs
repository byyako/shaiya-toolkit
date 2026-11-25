//! The health window of a loose data folder: what `shaiya loose health` reports (files that cannot be read, model and SData files that fail
//! their reader, model textures found nowhere, leftover temporary files, journals) and the recovery of an interrupted change. The look is
//! read only; recovery is a write and goes through a confirmation with the Changes / Writes / Undo / Game rows.

use crate::explain::{self, Rows};
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::loose::LooseLayout;
use shaiya_archive::loose_edit::{self, Busy, LooseOptions};
use shaiya_clientdata::LooseTree;
use shaiya_inspect::loose::LooseHealth;
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::PathBuf;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

enum Load {
    Idle,
    Loading(TaskHandle<LooseHealth>),
    Ready(Box<LooseHealth>),
    Failed(String),
}

enum Flow {
    Idle,
    /// The recovery question; the flag is `Busy::Unknown` (the button then says the game is stopped).
    Confirm(bool),
    Recovering(TaskHandle<Vec<String>>),
}

/// What a frame of the window asks the app frame to do.
pub enum LooseHealthAction {
    /// A recovery ran (or failed): reload the open folder when it is this client's.
    Reload(PathBuf),
}

pub struct LooseHealthView {
    pub open: bool,
    client: Option<(String, LooseLayout)>,
    load: Load,
    flow: Flow,
    rows: Rows,
    /// The last thing a recovery did (or why it did not).
    pub last: Option<String>,
    multi_running: bool,
}

impl Default for LooseHealthView {
    fn default() -> Self {
        LooseHealthView { open: false, client: None, load: Load::Idle, flow: Flow::Idle, rows: Vec::new(), last: None, multi_running: false }
    }
}

impl LooseHealthView {
    /// Opens the window for a saved loose client and reads its report on a worker thread.
    pub fn open_for(&mut self, name: &str, layout: LooseLayout, cx: &mut ToolkitCx<'_>) {
        self.open = true;
        self.client = Some((name.to_string(), layout));
        self.last = None;
        self.reload(cx);
    }

    pub fn set_multi_running(&mut self, on: bool) {
        self.multi_running = on;
    }

    /// The report once it is read (for tests).
    pub fn report(&self) -> Option<&LooseHealth> {
        match &self.load {
            Load::Ready(h) => Some(h),
            _ => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.load, Load::Loading(_)) || !matches!(self.flow, Flow::Idle)
    }

    fn reload(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some((_, layout)) = &self.client else { return };
        let layout = layout.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Looking at the loose folder".into(), cancellable: true, exclusive: None }, move |job| {
            let tree = LooseTree::open(&layout, job)?;
            shaiya_inspect::loose::health(&layout, &tree, job)
        });
        self.load = Load::Loading(h);
    }

    /// The recovery question (tests press it through `confirm_recover`).
    pub fn ask_recover(&mut self) {
        let Some((_, layout)) = &self.client else { return };
        if self.is_busy() || self.multi_running || loose_edit::unfinished(layout).is_none() {
            return;
        }
        let busy = loose_edit::busy_state(layout);
        self.rows = vec![
            (explain::CHANGES.to_string(), "puts the data folder back as it was before the interrupted change: files it replaced or removed come back from the journal, files it created are deleted".to_string()),
            (explain::WRITES.to_string(), format!("the data folder {}; the journal in {} is marked as not made", layout.data.display(), layout.backups.as_ref().map(|b| b.display().to_string()).unwrap_or_default())),
            (explain::UNDO.to_string(), "none needed: the folder is as it was before the change".to_string()),
            (explain::GAME.to_string(), explain::loose_game_text(&busy)),
        ];
        self.flow = Flow::Confirm(busy == Busy::Unknown);
    }

    fn poll(&mut self, cx: &mut ToolkitCx<'_>, actions: &mut Vec<LooseHealthAction>) {
        if let Load::Loading(h) = &mut self.load {
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done(r)) => self.load = Load::Ready(Box::new(r)),
                Some(TaskOutcome::Failed(e)) => self.load = Load::Failed(e.to_string()),
                Some(TaskOutcome::Cancelled) => self.load = Load::Failed("cancelled".to_string()),
                Some(TaskOutcome::Panicked(m)) => self.load = Load::Failed(format!("internal error (a bug): {m}")),
            }
        }
        match std::mem::replace(&mut self.flow, Flow::Idle) {
            Flow::Idle => {}
            Flow::Confirm(unknown) => {
                let name = self.client.as_ref().map(|(n, _)| n.clone()).unwrap_or_default();
                let button = if unknown { "The game is stopped: recover" } else { "Recover" };
                let c = Confirm { title: "Recover an interrupted change".into(), text: format!("Put {name} right after the change that did not finish?"), buttons: vec![button.into(), "Cancel".into()], danger: Some(0), details: self.rows.clone() };
                match cx.shell.confirm("toolkit-loose-recover", &c) {
                    None => self.flow = Flow::Confirm(unknown),
                    Some(0) => {
                        let Some((_, layout)) = self.client.clone() else { return };
                        let key = layout.root.display().to_string();
                        let h = cx.shell.tasks().spawn(TaskSpec { title: "Recovering".into(), cancellable: false, exclusive: Some(key) }, move |_job| {
                            loose_edit::recover(&layout, &LooseOptions { confirmed_unknown_busy: unknown, ..LooseOptions::default() })
                        });
                        self.flow = Flow::Recovering(h);
                    }
                    Some(_) => {}
                }
            }
            Flow::Recovering(mut h) => match h.poll() {
                None => self.flow = Flow::Recovering(h),
                Some(outcome) => {
                    self.last = Some(match outcome {
                        TaskOutcome::Done(notes) if notes.is_empty() => "Nothing was left to recover.".to_string(),
                        TaskOutcome::Done(notes) => notes.join(" "),
                        TaskOutcome::Failed(e) => format!("Not recovered: {e}"),
                        TaskOutcome::Cancelled => "Cancelled.".to_string(),
                        TaskOutcome::Panicked(m) => format!("Not recovered: internal error (a bug): {m}"),
                    });
                    if let Some((_, layout)) = &self.client {
                        actions.push(LooseHealthAction::Reload(layout.root.clone()));
                    }
                    self.reload(cx);
                }
            },
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) -> Vec<LooseHealthAction> {
        let mut actions = Vec::new();
        self.poll(cx, &mut actions);
        if self.is_busy() {
            ctx.request_repaint();
        }
        if self.open {
            self.window(ctx, cx);
        }
        actions
    }

    fn window(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) {
        let pal = cx.shell.pal();
        let busy = self.is_busy();
        let mut open = self.open;
        let (mut again, mut recover) = (false, false);
        let name = self.client.as_ref().map(|(n, _)| n.clone()).unwrap_or_default();
        egui::Window::new("Loose folder health").open(&mut open).default_width(720.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&name).strong());
                if ui.add_enabled(!busy, egui::Button::new("Check again")).clicked() {
                    again = true;
                }
            });
            ui.label(RichText::new("This only looks. Nothing in the folder is changed unless you confirm a recovery.").color(pal.weak));
            ui.separator();
            match &self.load {
                Load::Idle => {
                    ui.label("Choose a loose client first.");
                }
                Load::Loading(_) => {
                    ui.label("Checking...");
                }
                Load::Failed(why) => {
                    ui.label(RichText::new(format!("The folder cannot be read: {why}")).color(pal.error));
                }
                Load::Ready(h) => {
                    ui.label(format!("{} listed.", shaiya_inspect::counted(h.files as u64, "file", "files")));
                    let line = |ui: &mut egui::Ui, n: usize, what: &str| {
                        let color = if n == 0 { pal.ok } else { pal.warning };
                        ui.label(RichText::new(format!("{n} {what}")).color(color));
                    };
                    line(ui, h.unreadable, "files cannot be read");
                    for p in &h.unreadable_list {
                        ui.label(format!("    {p}"));
                    }
                    line(ui, h.failing, "model or SData files fail their reader");
                    for (p, why) in &h.failing_list {
                        ui.label(format!("    {p}: {why}"));
                    }
                    line(ui, h.unresolved, "model textures are found nowhere (as .dds, .tga, .png or _dds.png)");
                    for (m, t) in &h.unresolved_list {
                        ui.label(format!("    {m}: {t}"));
                    }
                    if h.lists_refused > 0 {
                        line(ui, h.lists_refused, "list files (.MLT, .MON, .ITM) could not be read");
                    }
                    line(ui, h.temps, "temporary files of the toolkit are left over");
                    if let Some((all, possible)) = h.journals {
                        ui.label(format!("{} in the backup folder, {} can be rolled back.", shaiya_inspect::counted(all as u64, "journal", "journals"), possible));
                    }
                    for n in &h.notes {
                        ui.label(RichText::new(n).color(pal.weak));
                    }
                    if let Some(u) = &h.unfinished {
                        ui.separator();
                        ui.label(RichText::new(u).color(pal.warning));
                        if ui.add_enabled(!busy && !self.multi_running, egui::Button::new("Recover...")).on_hover_text("Undoes the change that did not finish, from its journal").clicked() {
                            recover = true;
                        }
                    }
                }
            }
            if let Some(l) = &self.last {
                ui.separator();
                ui.label(l);
            }
        });
        self.open = open;
        if again {
            self.reload(cx);
        }
        if recover {
            self.ask_recover();
        }
    }
}
