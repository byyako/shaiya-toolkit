//! The SData table view: a window that shows an SData file as rows and columns (virtual scrolling), with a filter, the row count,
//! Copy rows and Export CSV. Reading happens on a worker thread; a file no layout fits is shown as hex with a sentence saying so.
//! Read only: nothing here writes to a client or to the file.

use crate::preview;
use crate::ToolkitSettings;
use egui::{Align2, FontId, Id, RichText, Sense};
use shaiya_core::text::Encoding;
use shaiya_core::{Error, Job, Result};
use shaiya_sdata::{Hint, ReadError, Table};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::Path;
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// How much of an unreadable file the hex fallback shows.
const HEX_HEAD: usize = 16 << 20;

/// Reads the file's bytes on the worker thread.
pub type Loader = Box<dyn FnOnce(&Job) -> Result<Vec<u8>> + Send>;

/// Where the bytes of the file come from.
pub enum Input {
    /// The file as stored (SEED-wrapped or plain).
    Bytes(Vec<u8>),
    /// Read on the worker thread.
    Load(Loader),
    /// Bytes the view already holds (a code page change reads them again).
    Shared(Arc<Vec<u8>>),
}

enum LoadKind {
    Table { table: Box<Table>, hay: Vec<Vec<String>> },
    Raw { message: String, total: usize, head: Vec<u8> },
}

struct LoadOut {
    raw: Arc<Vec<u8>>,
    kind: LoadKind,
}

enum State {
    Loading(TaskHandle<LoadOut>),
    Table { table: Box<Table>, hay: Vec<Vec<String>> },
    Raw { message: String, total: usize, head: Vec<u8> },
    Failed(String),
}

fn load(input: Input, name: &str, enc: Encoding, job: &Job) -> Result<LoadOut> {
    let raw = match input {
        Input::Bytes(b) => Arc::new(b),
        Input::Shared(a) => a,
        Input::Load(f) => Arc::new(f(job)?),
    };
    job.check()?;
    match shaiya_sdata::read_file(&raw, name, Hint::Auto, enc) {
        Ok(table) => {
            let mut hay = Vec::with_capacity(table.sheets.len());
            for s in &table.sheets {
                job.check()?;
                hay.push(
                    s.rows
                        .iter()
                        .map(|r| r.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\u{1}").to_lowercase())
                        .collect::<Vec<_>>(),
                );
            }
            Ok(LoadOut { raw, kind: LoadKind::Table { table: Box::new(table), hay } })
        }
        Err(ReadError::NoFit(n)) => {
            let plain = shaiya_seed::unwrap(&raw, name).map(|p| p.0).unwrap_or_else(|_| raw.to_vec());
            let head = plain.iter().take(HEX_HEAD).copied().collect();
            Ok(LoadOut { raw, kind: LoadKind::Raw { message: format!("{n}; shown as hex"), total: plain.len(), head } })
        }
        Err(ReadError::Unreadable(e)) => {
            let head = raw.iter().take(HEX_HEAD).copied().collect();
            Ok(LoadOut { kind: LoadKind::Raw { message: format!("{e}; shown as hex"), total: raw.len(), head }, raw })
        }
    }
}

pub struct TableView {
    pub title: String,
    pub enc: Encoding,
    state: State,
    raw: Option<Arc<Vec<u8>>>,
    sheet: usize,
    pub filter: String,
    /// The rows (indices into the sheet) the filter lets through, for the sheet and filter they were made for.
    shown: Vec<u32>,
    shown_for: Option<(usize, String)>,
    widths: Vec<Option<Vec<f32>>>,
    open: bool,
}

impl TableView {
    pub fn open(title: String, input: Input, cx: &mut ToolkitCx<'_>) -> TableView {
        let mut v = TableView { title, enc: Encoding::Cp949, state: State::Failed(String::new()), raw: None, sheet: 0, filter: String::new(), shown: Vec::new(), shown_for: None, widths: Vec::new(), open: true };
        v.start(input, cx);
        v
    }

    fn start(&mut self, input: Input, cx: &mut ToolkitCx<'_>) {
        let (name, enc) = (self.title.clone(), self.enc);
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Reading the table of {name}"), cancellable: true, exclusive: None }, move |job| load(input, &name, enc, job));
        self.state = State::Loading(h);
    }

    /// Reads the same bytes again with another code page.
    pub fn set_encoding(&mut self, enc: Encoding, cx: &mut ToolkitCx<'_>) {
        if enc == self.enc {
            return;
        }
        self.enc = enc;
        if let Some(raw) = self.raw.clone() {
            self.start(Input::Shared(raw), cx);
        }
    }

    pub fn tick(&mut self) {
        let State::Loading(h) = &mut self.state else { return };
        let Some(out) = h.poll() else { return };
        self.state = match out {
            TaskOutcome::Done(o) => {
                self.raw = Some(o.raw);
                match o.kind {
                    LoadKind::Table { table, hay } => {
                        self.sheet = table.layout.primary();
                        self.widths = vec![None; table.sheets.len()];
                        self.shown_for = None;
                        State::Table { table, hay }
                    }
                    LoadKind::Raw { message, total, head } => State::Raw { message, total, head },
                }
            }
            TaskOutcome::Failed(e) => State::Failed(e.to_string()),
            TaskOutcome::Cancelled => State::Failed("cancelled".to_string()),
            TaskOutcome::Panicked(m) => State::Failed(format!("internal error (a bug): {m}")),
        };
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    pub fn is_table(&self) -> bool {
        matches!(self.state, State::Table { .. })
    }

    /// The message of the hex fallback ("No table layout fits this file (N bytes); shown as hex").
    pub fn raw_message(&self) -> Option<&str> {
        match &self.state {
            State::Raw { message, .. } => Some(message),
            State::Failed(m) => Some(m),
            _ => None,
        }
    }

    pub fn layout_name(&self) -> Option<&str> {
        match &self.state {
            State::Table { table, .. } => Some(&table.layout.variant),
            _ => None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn sheet_of(&self) -> Option<&shaiya_sdata::Sheet> {
        match &self.state {
            State::Table { table, .. } => table.sheets.get(self.sheet),
            _ => None,
        }
    }

    pub fn sheet_names(&self) -> Vec<String> {
        match &self.state {
            State::Table { table, .. } => table.sheets.iter().map(|s| s.name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn select_sheet(&mut self, name: &str) {
        if let State::Table { table, .. } = &self.state {
            if let Some(i) = table.sheets.iter().position(|s| s.name == name) {
                self.sheet = i;
            }
        }
    }

    /// Rows in the shown sheet.
    pub fn row_count(&self) -> usize {
        self.sheet_of().map_or(0, |s| s.rows.len())
    }

    /// Rows the filter lets through (after [`TableView::refresh`]).
    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    /// Applies the filter when it or the sheet changed. Every whitespace-separated word must occur in the row (any case).
    pub fn refresh(&mut self) {
        let want = (self.sheet, self.filter.clone());
        if self.shown_for.as_ref() == Some(&want) {
            return;
        }
        let State::Table { hay, table } = &self.state else { return };
        let terms: Vec<String> = self.filter.split_whitespace().map(str::to_lowercase).collect();
        let total = table.sheets.get(self.sheet).map_or(0, |s| s.rows.len());
        self.shown = if terms.is_empty() {
            (0..total as u32).collect()
        } else {
            hay.get(self.sheet).map(|h| h.iter().enumerate().filter(|(_, line)| terms.iter().all(|t| line.contains(t.as_str()))).map(|(i, _)| i as u32).collect()).unwrap_or_default()
        };
        self.shown_for = Some(want);
    }

    pub fn set_filter(&mut self, f: &str) {
        self.filter = f.to_string();
        self.refresh();
    }

    /// The header and the shown rows, tab separated, one line per row (tabs and line breaks inside a cell become spaces).
    pub fn rows_tsv(&self) -> String {
        let Some(s) = self.sheet_of() else { return String::new() };
        let clean = |t: String| t.replace(['\t', '\r', '\n'], " ");
        let mut out = s.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join("\t");
        out.push('\n');
        for &i in &self.shown {
            out.push_str(&s.rows[i as usize].iter().map(|v| clean(v.to_string())).collect::<Vec<_>>().join("\t"));
            out.push('\n');
        }
        out
    }

    /// The shown rows as CSV (the format of `shaiya sdata table --csv`).
    pub fn csv(&self) -> Option<String> {
        let s = self.sheet_of()?;
        Some(shaiya_sdata::csv_rows(s, self.shown.iter().map(|&i| &s.rows[i as usize])))
    }

    pub fn export_csv(&self, path: &Path) -> Result<usize> {
        let text = self.csv().ok_or_else(|| Error::Refused("there is no table to export".into()))?;
        shaiya_core::fs::write_atomic(path, text.as_bytes())?;
        Ok(self.shown.len())
    }

    fn column_widths(&mut self) -> Vec<f32> {
        if let Some(Some(w)) = self.widths.get(self.sheet) {
            return w.clone();
        }
        let Some(s) = self.sheet_of() else { return Vec::new() };
        let w: Vec<f32> = (0..s.columns.len())
            .map(|c| {
                let longest = s.rows.iter().take(300).map(|r| r[c].to_string().chars().count()).max().unwrap_or(0).max(s.columns[c].name.chars().count());
                (longest as f32 * 7.5 + 16.0).clamp(48.0, 320.0)
            })
            .collect();
        if let Some(slot) = self.widths.get_mut(self.sheet) {
            *slot = Some(w.clone());
        }
        w
    }

    /// Draws the window. Returns false once it was closed.
    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) -> bool {
        self.tick();
        self.refresh();
        let mut open = self.open;
        let title = format!("SData table: {}", preview::file_name(&self.title));
        egui::Window::new(title).id(Id::unique("toolkit-sdata-table")).default_size([980.0, 560.0]).resizable(true).collapsible(false).open(&mut open).show(ctx, |ui| self.body(ui, cx));
        self.open = open;
        self.open
    }

    fn body(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) {
        let pal = cx.shell.pal();
        if self.is_loading() {
            ui.label("Reading the table...");
            return;
        }
        let mut new_enc = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("Names read as");
            let mut chosen = self.enc;
            egui::ComboBox::from_id_salt("toolkit-sdata-table-enc").selected_text(shaiya_sdata::encoding_name(self.enc)).show_ui(ui, |ui| {
                for e in shaiya_sdata::ENCODINGS {
                    ui.selectable_value(&mut chosen, e, shaiya_sdata::encoding_name(e));
                }
            });
            if chosen != self.enc {
                new_enc = Some(chosen);
            }
        });
        if let Some(e) = new_enc {
            self.set_encoding(e, cx);
            return;
        }
        match &self.state {
            State::Failed(m) => {
                ui.label(RichText::new(m).color(pal.error));
                return;
            }
            State::Raw { message, total, head } => {
                ui.label(RichText::new(message).color(pal.warning));
                if head.len() < *total {
                    ui.label(RichText::new(format!("showing the first {} of {} bytes", shaiya_inspect::group(head.len() as u64), shaiya_inspect::group(*total as u64))).color(pal.weak));
                }
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, preview::hex_rows(head.len()), |ui, range| {
                    for r in range {
                        ui.label(RichText::new(preview::hex_row(head, 0, r)).monospace());
                    }
                });
                return;
            }
            _ => {}
        }
        // the table
        let names = self.sheet_names();
        let layout = self.layout_name().unwrap_or("").to_string();
        let mut copy = false;
        let mut export = false;
        let mut new_sheet = None;
        ui.horizontal_wrapped(|ui| {
            if names.len() > 1 {
                egui::ComboBox::from_id_salt("toolkit-sdata-table-sheet").selected_text(names.get(self.sheet).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(i == self.sheet, n).clicked() {
                            new_sheet = Some(i);
                        }
                    }
                });
            }
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter rows").desired_width(200.0));
            let total = self.row_count();
            let shown = self.shown.len();
            ui.label(if shown == total { format!("{} rows", shaiya_inspect::group(total as u64)) } else { format!("{} of {} rows", shaiya_inspect::group(shown as u64), shaiya_inspect::group(total as u64)) });
            if ui.button("Copy rows").on_hover_text("Copies the shown rows, tab separated, with the column names").clicked() {
                copy = true;
            }
            if ui.button("Export CSV...").on_hover_text("Writes the shown rows to a CSV file you choose").clicked() {
                export = true;
            }
        });
        ui.label(RichText::new(format!("Layout {layout}. Read only; the file is not changed.")).color(pal.weak));
        if let Some(i) = new_sheet {
            self.sheet = i;
        }
        if copy {
            ui.ctx().copy_text(self.rows_tsv());
            cx.shell.status(format!("Copied {}", shaiya_inspect::counted(self.shown.len() as u64, "row", "rows")));
        }
        if export {
            let default = format!("{}.csv", self.sheet_names().get(self.sheet).cloned().unwrap_or_else(|| "table".into()));
            if let Some(p) = cx.shell.save_file("sdata-csv", &default, &[("CSV", &["csv"])]) {
                match self.export_csv(&p) {
                    Ok(n) => cx.shell.status(format!("Wrote {} to {}", shaiya_inspect::counted(n as u64, "row", "rows"), p.display())),
                    Err(e) => cx.shell.error("Export CSV failed", &e),
                }
            }
        }
        self.refresh();
        self.draw_grid(ui, cx);
    }

    fn draw_grid(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) {
        let widths = self.column_widths();
        let total_w: f32 = widths.iter().sum();
        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let body_h = (ui.available_height() - row_h - 8.0).max(row_h * 3.0);
        let State::Table { table, .. } = &self.state else { return };
        let Some(sheet) = table.sheets.get(self.sheet) else { return };
        let shown = &self.shown;
        let font = FontId::proportional(13.0);
        let text_color = ui.visuals().text_color();
        let head_color = ui.visuals().strong_text_color();
        let stripe = ui.visuals().faint_bg_color;
        let mut cjk = false;
        egui::ScrollArea::horizontal().id_salt("toolkit-sdata-table-h").auto_shrink([false, false]).show(ui, |ui| {
            let (hrect, _) = ui.allocate_exact_size(egui::vec2(total_w, row_h), Sense::hover());
            let mut x = hrect.left();
            for (c, w) in widths.iter().enumerate() {
                let cell = egui::Rect::from_min_size(egui::pos2(x, hrect.top()), egui::vec2(*w, row_h));
                ui.painter_at(cell).text(egui::pos2(x + 6.0, cell.center().y), Align2::LEFT_CENTER, &sheet.columns[c].name, FontId::proportional(13.0), head_color);
                x += *w;
            }
            ui.allocate_ui(egui::vec2(total_w, body_h), |ui| {
                egui::ScrollArea::vertical().id_salt("toolkit-sdata-table-v").auto_shrink([false, false]).max_height(body_h).show_rows(ui, row_h, shown.len(), |ui, range| {
                    for i in range {
                        let row = &sheet.rows[shown[i] as usize];
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(total_w, row_h), Sense::hover());
                        if i % 2 == 1 {
                            ui.painter().rect_filled(rect, 0.0, stripe);
                        }
                        let mut x = rect.left();
                        for (c, w) in widths.iter().enumerate() {
                            let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(*w, row_h));
                            let text = row[c].to_string();
                            if !cjk && text.chars().any(|ch| (ch as u32) > 0x2E80) {
                                cjk = true;
                            }
                            ui.painter_at(cell).text(egui::pos2(x + 6.0, cell.center().y), Align2::LEFT_CENTER, text, font.clone(), text_color);
                            x += *w;
                        }
                    }
                });
            });
        });
        if cjk {
            cx.shell.need_cjk_font();
        }
    }
}
