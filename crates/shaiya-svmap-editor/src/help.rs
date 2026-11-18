//! The Help window: `docs/tools/svmap-editor.md` rendered in the app. The guide is the single source of the help text; a small
//! markdown renderer (headings, paragraphs, lists, pipe tables, fenced code, notes, `code`, **bold**, *italic*, links, pictures
//! as "[picture: alt]") draws it, and two pages are generated from the code instead of read from the file: "Keyboard shortcuts"
//! (the command table) and "Problems explained" (the rule table). The guide carries the same two tables in static form and tests
//! check they agree. A bold `**Menu > Item**` that is a command's menu path is drawn as a link that runs the command.

use std::sync::OnceLock;

use egui::{text::LayoutJob, Align, Color32, FontId, RichText, TextFormat};
use shaiya_svmap::SectionId;

use crate::commands::{self, Cmd};
use crate::help_text;
use crate::view2d::{DrawTarget, Tool};

pub const GUIDE_MD: &str = include_str!("../../../docs/tools/svmap-editor.md");

// ------------------------------------------------------------------------------------------------------------------ model

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Bold(String),
    Italic(String),
    Link { text: String, target: String },
    Picture(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading { level: u8, text: Vec<Inline>, anchor: String },
    Para(Vec<Inline>),
    /// (nesting 0 or 1, number for an ordered list, text)
    ListItem { nest: u8, number: Option<u32>, text: Vec<Inline> },
    Table { header: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Code(String),
    Note(Vec<Inline>),
}

#[derive(Clone, Debug)]
pub struct Page {
    pub title: String,
    pub anchor: String,
    /// Level-3 headings of the page: (title, anchor).
    pub subs: Vec<(String, String)>,
    pub blocks: Vec<Block>,
}

pub struct Guide {
    pub title: String,
    pub pages: Vec<Page>,
    /// Lines the renderer cannot draw (a test requires none in the guide).
    pub unsupported: Vec<String>,
}

pub fn plain(inlines: &[Inline]) -> String {
    let mut s = String::new();
    for i in inlines {
        match i {
            Inline::Text(t) | Inline::Code(t) | Inline::Bold(t) | Inline::Italic(t) => s.push_str(t),
            Inline::Link { text, .. } => s.push_str(text),
            Inline::Picture(alt) => s.push_str(&format!("[picture: {alt}]")),
        }
    }
    s
}

/// GitHub-style anchor: lower case, letters and digits kept, spaces and hyphens become hyphens, the rest dropped.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if c == ' ' || c == '-' {
            out.push('-');
        }
    }
    out
}

/// Inline markup of one piece of text. `bad` collects what cannot be drawn.
pub fn parse_inline(s: &str, bad: &mut Vec<String>) -> Vec<Inline> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut Vec<Inline>| {
        if !text.is_empty() {
            out.push(Inline::Text(std::mem::take(text)));
        }
    };
    let find = |from: usize, pat: &[char]| -> Option<usize> { (from..chars.len().saturating_sub(pat.len() - 1)).find(|&i| chars[i..i + pat.len()] == *pat) };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(j) = find(i + 1, &['`']) {
                flush(&mut text, &mut out);
                out.push(Inline::Code(chars[i + 1..j].iter().collect()));
                i = j + 1;
                continue;
            }
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(j) = find(i + 2, &['*', '*']) {
                flush(&mut text, &mut out);
                out.push(Inline::Bold(chars[i + 2..j].iter().collect()));
                i = j + 2;
                continue;
            }
        }
        if c == '*' && chars.get(i + 1).is_some_and(|n| !n.is_whitespace() && *n != '*') {
            if let Some(j) = find(i + 1, &['*']) {
                flush(&mut text, &mut out);
                out.push(Inline::Italic(chars[i + 1..j].iter().collect()));
                i = j + 1;
                continue;
            }
        }
        if c == '!' && chars.get(i + 1) == Some(&'[') || c == '[' {
            let start = if c == '!' { i + 1 } else { i };
            if let Some(close) = find(start + 1, &[']']) {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = find(close + 2, &[')']) {
                        let label: String = chars[start + 1..close].iter().collect();
                        let target: String = chars[close + 2..end].iter().collect();
                        flush(&mut text, &mut out);
                        out.push(if c == '!' { Inline::Picture(label) } else { Inline::Link { text: label, target } });
                        i = end + 1;
                        continue;
                    }
                }
            }
            bad.push(format!("a bracket that is not a link: {s}"));
        }
        if c == '<' && chars.get(i + 1).is_some_and(|n| n.is_alphabetic() || *n == '/') && find(i + 1, &['>']).is_some() {
            bad.push(format!("html: {s}"));
        }
        if c == '~' && chars.get(i + 1) == Some(&'~') {
            bad.push(format!("strikethrough: {s}"));
        }
        text.push(c);
        i += 1;
    }
    flush(&mut text, &mut out);
    out
}

fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_table_sep(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn list_marker(line: &str) -> Option<(usize, Option<u32>, &str)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return Some((indent, None, r));
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 3 && t[digits..].starts_with(". ") {
        return Some((indent, t[..digits].parse().ok(), &t[digits + 2..]));
    }
    None
}

/// Parses the guide: blocks, then pages (one per level-2 heading; text before the first one is a page of its own).
pub fn parse(md: &str) -> Guide {
    let mut bad: Vec<String> = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut i = 0;
    let mut para: Vec<String> = Vec::new();
    let flush_para = |para: &mut Vec<String>, blocks: &mut Vec<Block>, bad: &mut Vec<String>| {
        if !para.is_empty() {
            let joined = para.join(" ");
            blocks.push(Block::Para(parse_inline(&joined, bad)));
            para.clear();
        }
    };
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_end();
        if t.trim().is_empty() {
            flush_para(&mut para, &mut blocks, &mut bad);
            i += 1;
            continue;
        }
        if t.starts_with("```") {
            flush_para(&mut para, &mut blocks, &mut bad);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_end().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code(code.join("\n")));
            continue;
        }
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=4).contains(&hashes) && t[hashes..].starts_with(' ') {
            flush_para(&mut para, &mut blocks, &mut bad);
            let text = parse_inline(t[hashes + 1..].trim(), &mut bad);
            let anchor = slug(&plain(&text));
            blocks.push(Block::Heading { level: hashes as u8, text, anchor });
            i += 1;
            continue;
        }
        if hashes > 4 && t[hashes..].starts_with(' ') {
            bad.push(format!("heading deeper than level 4: {t}"));
        }
        if t.starts_with('>') {
            flush_para(&mut para, &mut blocks, &mut bad);
            let mut text = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start().trim_start_matches('>').trim();
                if !l.is_empty() {
                    text.push(l.to_string());
                }
                i += 1;
            }
            blocks.push(Block::Note(parse_inline(&text.join(" "), &mut bad)));
            continue;
        }
        if t.trim_start().starts_with('|') && i + 1 < lines.len() && is_table_sep(lines[i + 1]) {
            flush_para(&mut para, &mut blocks, &mut bad);
            let header: Vec<Vec<Inline>> = split_row(t).iter().map(|c| parse_inline(c, &mut bad)).collect();
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                let cells = split_row(lines[i]);
                if cells.len() != header.len() {
                    bad.push(format!("table row with {} cells under a header of {}: {}", cells.len(), header.len(), lines[i]));
                }
                rows.push(cells.iter().map(|c| parse_inline(c, &mut bad)).collect());
                i += 1;
            }
            blocks.push(Block::Table { header, rows });
            continue;
        }
        if let Some((indent, number, rest)) = list_marker(t) {
            flush_para(&mut para, &mut blocks, &mut bad);
            let nest = if indent >= 2 { 1 } else { 0 };
            if indent >= 6 {
                bad.push(format!("list nested deeper than one level: {t}"));
            }
            let mut text = rest.trim().to_string();
            i += 1;
            // continuation lines: indented, not a new item
            while i < lines.len() && !lines[i].trim().is_empty() && lines[i].starts_with("  ") && list_marker(lines[i]).is_none() {
                text.push(' ');
                text.push_str(lines[i].trim());
                i += 1;
            }
            blocks.push(Block::ListItem { nest, number, text: parse_inline(&text, &mut bad) });
            continue;
        }
        if t.starts_with('<') {
            bad.push(format!("html: {t}"));
        } else if t.starts_with("    ") && para.is_empty() {
            bad.push(format!("indented code block: {t}"));
        } else if t.chars().all(|c| c == '-' || c == '=' || c == '*' || c == '_') && t.len() >= 3 {
            bad.push(format!("rule or setext heading: {t}"));
        } else if t.starts_with("~~~") {
            bad.push(format!("tilde fence: {t}"));
        }
        para.push(t.trim().to_string());
        i += 1;
    }
    flush_para(&mut para, &mut blocks, &mut bad);

    // pages
    let mut title = String::new();
    let mut pages: Vec<Page> = Vec::new();
    let mut intro: Vec<Block> = Vec::new();
    for b in blocks {
        match &b {
            Block::Heading { level: 1, text, .. } if title.is_empty() => title = plain(text),
            Block::Heading { level: 2, text, anchor } => pages.push(Page { title: plain(text), anchor: anchor.clone(), subs: Vec::new(), blocks: vec![b.clone()] }),
            _ => match pages.last_mut() {
                Some(p) => {
                    if let Block::Heading { level: 3, text, anchor } = &b {
                        p.subs.push((plain(text), anchor.clone()));
                    }
                    p.blocks.push(b);
                }
                None => intro.push(b),
            },
        }
    }
    if !intro.is_empty() {
        pages.insert(0, Page { title: "About this guide".to_string(), anchor: "about-this-guide".to_string(), subs: Vec::new(), blocks: intro });
    }
    Guide { title, pages, unsupported: bad }
}

// ------------------------------------------------------------------------------------------------------------------ generated pages

pub const SHORTCUTS_TITLE: &str = "Keyboard shortcuts";
pub const RULES_TITLE: &str = "Problems explained";

/// The shortcut table as markdown: one row per command that has keys, then the fixed keys. The guide holds exactly this text.
pub fn shortcuts_markdown() -> String {
    let mut s = String::from("| Keys | What it does |\n|---|---|\n");
    for i in commands::all().iter().filter(|i| !i.keys.is_empty()) {
        let keys: Vec<String> = i.keys.iter().map(|k| format!("`{k}`")).collect();
        let what = if i.menu.is_empty() { format!("{}: {}", i.label, i.help) } else { format!("**{}**: {}", i.menu, i.help) };
        s.push_str(&format!("| {} | {} |\n", keys.join(" or "), what));
    }
    for (k, what) in help_text::FIXED_KEYS {
        s.push_str(&format!("| `{k}` | {what} |\n"));
    }
    s
}

/// The rule table as markdown (the guide holds exactly this text).
pub fn rules_markdown() -> String {
    let mut s = String::from("| Rule | Kind | What it means | How to fix it |\n|---|---|---|---|\n");
    for r in help_text::RULES {
        s.push_str(&format!("| `{}` | {} | {}. {} | {} |\n", r.code, r.severity, r.title, r.why, r.fix));
    }
    s
}

fn table_blocks(md: &str) -> Vec<Block> {
    let g = parse(&format!("## x\n\n{md}"));
    debug_assert!(g.unsupported.is_empty());
    g.pages.into_iter().next().map(|p| p.blocks.into_iter().skip(1).collect()).unwrap_or_default()
}

fn build_guide() -> Guide {
    let mut g = parse(GUIDE_MD);
    for (title, md) in [(SHORTCUTS_TITLE, shortcuts_markdown()), (RULES_TITLE, rules_markdown())] {
        let mut blocks = vec![Block::Heading { level: 2, text: vec![Inline::Text(title.to_string())], anchor: slug(title) }];
        blocks.extend(table_blocks(&md));
        match g.pages.iter_mut().find(|p| p.title == title) {
            Some(p) => {
                // keep the guide's own words above the table, replace the table itself
                let intro: Vec<Block> = p.blocks.iter().skip(1).take_while(|b| !matches!(b, Block::Table { .. })).cloned().collect();
                let mut nb = vec![blocks[0].clone()];
                nb.extend(intro);
                nb.extend(blocks.into_iter().skip(1));
                p.blocks = nb;
            }
            None => g.pages.push(Page { title: title.to_string(), anchor: slug(title), subs: Vec::new(), blocks }),
        }
    }
    g
}

pub fn guide() -> &'static Guide {
    static G: OnceLock<Guide> = OnceLock::new();
    G.get_or_init(build_guide)
}

// ------------------------------------------------------------------------------------------------------------------ F1 targets

/// The anchor F1 opens for the current state: the active tool, else the selected section, else the start.
pub fn anchor_for(tool: Tool, draw: DrawTarget, selected: Option<SectionId>) -> &'static str {
    match tool {
        Tool::Box => match draw {
            DrawTarget::MobArea => "monster-areas",
            DrawTarget::Bind => "bind-areas",
            DrawTarget::Region => "regions",
            DrawTarget::PointB => "sections-a-and-b",
        },
        Tool::Npc => "npcs",
        Tool::Portal => "portals",
        Tool::Paint => "blocked-cells",
        Tool::Measure => "measuring-distances",
        Tool::Scatter => "scatter-monster-areas",
        Tool::Place => "buildings-shapes-and-trees",
        Tool::Terrain => "terrain",
        Tool::Select => match selected {
            Some(SectionId::MobAreas) => "monster-areas",
            Some(SectionId::Npcs) => "npcs",
            Some(SectionId::Portals) => "portals",
            Some(SectionId::Binds) => "bind-areas",
            Some(SectionId::Regions) => "regions",
            Some(SectionId::SectionA | SectionId::SectionB) => "sections-a-and-b",
            Some(SectionId::Blocked) => "blocked-cells",
            Some(SectionId::Size) => "size",
            None => "getting-started",
        },
    }
}

// ------------------------------------------------------------------------------------------------------------------ the window

#[derive(Default)]
pub struct HelpState {
    pub page: usize,
    pub search: String,
    pub scroll_to: Option<String>,
}

impl HelpState {
    /// Shows the page that has this anchor (a page title or a heading below it) and scrolls to it. False when there is none.
    pub fn goto(&mut self, anchor: &str) -> bool {
        let g = guide();
        for (i, p) in g.pages.iter().enumerate() {
            if p.anchor == anchor || p.subs.iter().any(|(_, a)| a == anchor) {
                self.page = i;
                self.scroll_to = Some(anchor.to_string());
                self.search.clear();
                return true;
            }
        }
        false
    }

    /// The page with this title (or anchor, or number), as `--help-page` and `--show help:<page>` name it.
    pub fn goto_named(&mut self, name: &str) -> bool {
        if let Ok(n) = name.parse::<usize>() {
            self.page = n.min(guide().pages.len().saturating_sub(1));
            return true;
        }
        let slugged = slug(name);
        self.goto(&slugged)
    }
}

pub enum HelpAction {
    Run(Cmd),
}



fn text_format(ui: &egui::Ui, bold: bool, italic: bool, code: bool, hit: bool) -> TextFormat {
    let mut f = TextFormat::simple(if code { FontId::monospace(13.0) } else { FontId::proportional(14.0) }, ui.visuals().text_color());
    if bold {
        f.color = ui.visuals().strong_text_color();
    }
    f.italics = italic;
    if code {
        f.background = ui.visuals().code_bg_color;
    }
    if hit {
        f.background = Color32::from_rgba_unmultiplied(255, 200, 0, 90);
    }
    f
}

/// Adds `text` to a layout job, marking the parts that match the search.
fn add_text(ui: &egui::Ui, job: &mut LayoutJob, text: &str, bold: bool, italic: bool, code: bool, query: &str) {
    if query.is_empty() {
        job.append(text, 0.0, text_format(ui, bold, italic, code, false));
        return;
    }
    let (lt, lq) = (text.to_lowercase(), query.to_lowercase());
    // lower-casing can change byte lengths for a few scripts; fall back to no highlight then
    if lt.len() != text.len() {
        job.append(text, 0.0, text_format(ui, bold, italic, code, false));
        return;
    }
    let mut at = 0;
    while let Some(p) = lt[at..].find(&lq) {
        let s = at + p;
        if s > at {
            job.append(&text[at..s], 0.0, text_format(ui, bold, italic, code, false));
        }
        job.append(&text[s..s + lq.len()], 0.0, text_format(ui, bold, italic, code, true));
        at = s + lq.len();
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, text_format(ui, bold, italic, code, false));
    }
}

/// Draws a run of inline items, wrapped. Links and menu paths are separate widgets; plain runs are one label.
fn inlines(ui: &mut egui::Ui, items: &[Inline], query: &str, st: &mut HelpState, actions: &mut Vec<HelpAction>, can_run: &dyn Fn(Cmd) -> Result<(), &'static str>, size_boost: f32) {
    let interactive = items.iter().any(|i| match i {
        Inline::Link { .. } => true,
        Inline::Bold(t) => commands::by_menu_path(t).is_some(),
        _ => false,
    });
    if !interactive {
        let mut job = LayoutJob::default();
        job.wrap.max_width = ui.available_width();
        for i in items {
            match i {
                Inline::Text(t) => add_text(ui, &mut job, t, false, false, false, query),
                Inline::Code(t) => add_text(ui, &mut job, t, false, false, true, query),
                Inline::Bold(t) => add_text(ui, &mut job, t, true, false, false, query),
                Inline::Italic(t) => add_text(ui, &mut job, t, false, true, false, query),
                Inline::Picture(alt) => add_text(ui, &mut job, &format!("[picture: {alt}]"), false, true, false, query),
                Inline::Link { text, .. } => add_text(ui, &mut job, text, false, false, false, query),
            }
        }
        if size_boost != 0.0 {
            for s in &mut job.sections {
                s.format.font_id.size += size_boost;
            }
        }
        ui.add(egui::Label::new(job).wrap());
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for i in items {
            match i {
                Inline::Link { text, target } => {
                    let r = ui.add(egui::Button::new(RichText::new(text).color(crate::theme::pal_ui(ui).link).underline()).frame(false));
                    if let Some(anchor) = target.strip_prefix('#') {
                        if r.clicked() {
                            st.goto(anchor);
                        }
                    } else if target.starts_with("http://") || target.starts_with("https://") {
                        if r.on_hover_text(target).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(target));
                        }
                    } else {
                        r.on_hover_text(format!("{target} (a file in the repo's docs folder)"));
                    }
                }
                Inline::Bold(t) if commands::by_menu_path(t).is_some() => {
                    let info = commands::by_menu_path(t).expect("checked above");
                    let ok = can_run(info.cmd);
                    let label = RichText::new(t).strong().underline().color(if ok.is_ok() { crate::theme::pal_ui(ui).link } else { crate::theme::pal_ui(ui).weak });
                    let r = ui.add(egui::Button::new(label).frame(false));
                    match ok {
                        Ok(()) => {
                            if r.on_hover_text(format!("Click to run this command. {}", info.help)).clicked() {
                                actions.push(HelpAction::Run(info.cmd));
                            }
                        }
                        Err(why) => {
                            r.on_hover_text(format!("Not available now: {why}"));
                        }
                    }
                }
                other => {
                    let mut job = LayoutJob::default();
                    job.wrap.max_width = ui.available_width();
                    match other {
                        Inline::Text(t) => add_text(ui, &mut job, t, false, false, false, query),
                        Inline::Code(t) => add_text(ui, &mut job, t, false, false, true, query),
                        Inline::Bold(t) => add_text(ui, &mut job, t, true, false, false, query),
                        Inline::Italic(t) => add_text(ui, &mut job, t, false, true, false, query),
                        Inline::Picture(alt) => add_text(ui, &mut job, &format!("[picture: {alt}]"), false, true, false, query),
                        Inline::Link { .. } => {}
                    }
                    ui.add(egui::Label::new(job).wrap());
                }
            }
        }
    });
}

fn table(ui: &mut egui::Ui, header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], query: &str, st: &mut HelpState, actions: &mut Vec<HelpAction>, can_run: &dyn Fn(Cmd) -> Result<(), &'static str>) {
    let n = header.len().max(1);
    // column widths follow the length of the longest cell, limited so nothing is squeezed to nothing
    let mut weight = vec![6.0f32; n];
    for row in rows.iter().chain(std::iter::once(&header.to_vec())) {
        for (c, cell) in row.iter().enumerate().take(n) {
            weight[c] = weight[c].max((plain(cell).chars().count() as f32).min(60.0));
        }
    }
    let total: f32 = weight.iter().sum();
    let avail = (ui.available_width() - 8.0 * n as f32).max(200.0);
    let widths: Vec<f32> = weight.iter().map(|w| (avail * w / total).max(60.0)).collect();
    let row_ui = |ui: &mut egui::Ui, cells: &[Vec<Inline>], head: bool, st: &mut HelpState, actions: &mut Vec<HelpAction>| {
        ui.horizontal_top(|ui| {
            for (c, cell) in cells.iter().enumerate().take(n) {
                ui.allocate_ui_with_layout(egui::vec2(widths[c], 0.0), egui::Layout::top_down(Align::Min), |ui| {
                    ui.set_width(widths[c]);
                    if head {
                        let bold: Vec<Inline> = cell.iter().map(|i| Inline::Bold(plain(std::slice::from_ref(i)))).collect();
                        inlines(ui, &bold, query, st, actions, can_run, 0.0);
                    } else {
                        inlines(ui, cell, query, st, actions, can_run, 0.0);
                    }
                });
            }
        });
    };
    row_ui(ui, header, true, st, actions);
    ui.separator();
    for r in rows {
        row_ui(ui, r, false, st, actions);
        ui.add_space(2.0);
    }
}

fn block(ui: &mut egui::Ui, b: &Block, query: &str, st: &mut HelpState, actions: &mut Vec<HelpAction>, can_run: &dyn Fn(Cmd) -> Result<(), &'static str>) {
    match b {
        Block::Heading { level, text, anchor } => {
            ui.add_space(match level { 2 => 10.0, _ => 6.0 });
            let size = match level { 1 => 22.0, 2 => 19.0, 3 => 16.0, _ => 14.5 };
            let r = ui.label(RichText::new(plain(text)).size(size).strong());
            if st.scroll_to.as_deref() == Some(anchor.as_str()) {
                r.scroll_to_me(Some(Align::TOP));
                st.scroll_to = None;
            }
            ui.add_space(2.0);
        }
        Block::Para(t) => {
            inlines(ui, t, query, st, actions, can_run, 0.0);
            ui.add_space(5.0);
        }
        Block::ListItem { nest, number, text } => {
            ui.horizontal_top(|ui| {
                ui.add_space(8.0 + *nest as f32 * 18.0);
                ui.label(match number {
                    Some(n) => format!("{n}."),
                    None => "•".to_string(),
                });
                ui.vertical(|ui| inlines(ui, text, query, st, actions, can_run, 0.0));
            });
        }
        Block::Table { header, rows } => {
            table(ui, header, rows, query, st, actions, can_run);
            ui.add_space(6.0);
        }
        Block::Code(c) => {
            egui::Frame::new().fill(ui.visuals().code_bg_color).inner_margin(6.0).corner_radius(3.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(c).monospace().size(13.0)).wrap());
            });
            ui.add_space(5.0);
        }
        Block::Note(t) => {
            egui::Frame::new().fill(Color32::from_rgba_unmultiplied(120, 160, 255, 28)).inner_margin(6.0).corner_radius(3.0).show(ui, |ui| {
                inlines(ui, t, query, st, actions, can_run, 0.0);
            });
            ui.add_space(5.0);
        }
    }
}

fn page_matches(p: &Page, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    let in_inlines = |i: &[Inline]| plain(i).to_lowercase().contains(&q);
    p.title.to_lowercase().contains(&q)
        || p.blocks.iter().any(|b| match b {
            Block::Heading { text, .. } | Block::Para(text) | Block::Note(text) | Block::ListItem { text, .. } => in_inlines(text),
            Block::Code(c) => c.to_lowercase().contains(&q),
            Block::Table { header, rows } => header.iter().any(|c| in_inlines(c)) || rows.iter().any(|r| r.iter().any(|c| in_inlines(c))),
        })
}

/// The Help window. `can_run` tells whether a command is available (menu paths in the text run commands).
pub fn show(ctx: &egui::Context, open: &mut bool, st: &mut HelpState, can_run: &dyn Fn(Cmd) -> Result<(), &'static str>) -> Vec<HelpAction> {
    let g = guide();
    let mut actions = Vec::new();
    egui::Window::new("Help").open(open).default_size([780.0, 560.0]).min_size([420.0, 260.0]).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Search");
            ui.add(egui::TextEdit::singleline(&mut st.search).hint_text("words in the guide").desired_width(220.0));
            if !st.search.is_empty() && ui.small_button("clear").clicked() {
                st.search.clear();
            }
            ui.label(RichText::new(&g.title).weak());
        });
        ui.separator();
        let query = st.search.trim().to_string();
        let height = ui.available_height().max(200.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(190.0);
                egui::ScrollArea::vertical().id_salt("help-tree").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    let mut none = true;
                    for (i, p) in g.pages.iter().enumerate() {
                        if !page_matches(p, &query) {
                            continue;
                        }
                        none = false;
                        if ui.selectable_label(st.page == i, &p.title).clicked() {
                            st.page = i;
                            st.scroll_to = None;
                        }
                        if st.page == i {
                            for (t, a) in &p.subs {
                                if ui.add(egui::Button::new(RichText::new(format!("   {t}")).small()).frame(false)).clicked() {
                                    st.scroll_to = Some(a.clone());
                                }
                            }
                        }
                    }
                    if none {
                        ui.label("No page has those words.");
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                let page = g.pages.get(st.page.min(g.pages.len().saturating_sub(1)));
                egui::ScrollArea::vertical().id_salt(("help-page", st.page)).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match page {
                        Some(p) => {
                            for b in &p.blocks {
                                block(ui, b, &query, st, &mut actions, can_run);
                            }
                            if p.title == "About" {
                                ui.label(RichText::new(format!("Shaiya SVMap Editor version {}", env!("CARGO_PKG_VERSION"))).weak());
                            }
                        }
                        None => {
                            ui.label("The guide is empty.");
                        }
                    }
                });
            });
        });
    });
    actions
}

/// The `?` overlay: every key in two columns, from the command table.
pub fn shortcuts_overlay(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("Keyboard shortcuts").open(open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
            egui::Grid::new("shortcut-grid").striped(true).spacing([18.0, 3.0]).show(ui, |ui| {
                for i in commands::all().iter().filter(|i| !i.keys.is_empty()) {
                    ui.label(RichText::new(i.keys.join(" or ")).monospace());
                    ui.label(if i.menu.is_empty() { i.label.to_string() } else { commands::item_label(i).trim_end_matches("...").to_string() });
                    ui.end_row();
                }
                for (k, what) in help_text::FIXED_KEYS {
                    ui.label(RichText::new(*k).monospace());
                    ui.label(*what);
                    ui.end_row();
                }
            });
        });
        ui.label(RichText::new("Keys do nothing while a text field has the focus, except Ctrl+O, Ctrl+S, Ctrl+Z, Ctrl+Y, Ctrl+P, Ctrl+Shift+P, Ctrl+G, Ctrl+Shift+F and F1. Esc or ? closes this.").weak().small());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_inlines(g: &Guide) -> Vec<&Inline> {
        let mut v = Vec::new();
        for p in &g.pages {
            for b in &p.blocks {
                match b {
                    Block::Heading { text, .. } | Block::Para(text) | Block::Note(text) | Block::ListItem { text, .. } => v.extend(text.iter()),
                    Block::Table { header, rows } => {
                        v.extend(header.iter().flatten());
                        v.extend(rows.iter().flatten().flatten());
                    }
                    Block::Code(_) => {}
                }
            }
        }
        v
    }

    #[test]
    fn inline_markup_parses() {
        let mut bad = Vec::new();
        let v = parse_inline("Press `Ctrl+S` or **File > Save** *now*, see [the guide](#start) ![shot](a.png).", &mut bad);
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(v[1], Inline::Code("Ctrl+S".into()));
        assert_eq!(v[3], Inline::Bold("File > Save".into()));
        assert_eq!(v[5], Inline::Italic("now".into()));
        assert_eq!(v[7], Inline::Link { text: "the guide".into(), target: "#start".into() });
        assert_eq!(v[9], Inline::Picture("shot".into()));
        assert_eq!(plain(&v), "Press Ctrl+S or File > Save now, see the guide [picture: shot].");
        // what the renderer cannot draw is reported
        let mut bad = Vec::new();
        parse_inline("a <b>tag</b> and ~~gone~~ and [broken", &mut bad);
        for kind in ["html:", "strikethrough:", "a bracket that is not a link"] {
            assert!(bad.iter().any(|b| b.starts_with(kind)), "{kind} not reported in {bad:?}");
        }
        // markup inside code is left alone
        let mut bad = Vec::new();
        assert_eq!(parse_inline("`<number>.svmap` and `a*b*c`", &mut bad)[0], Inline::Code("<number>.svmap".into()));
        assert!(bad.is_empty());
    }

    #[test]
    fn blocks_and_pages_parse() {
        let md = "# Title\n\nintro line one\nline two\n\n## First page\n\ntext with `code`\n\n- a\n- b\n  continued\n  - nested\n1. one\n2. two\n\n### Sub heading\n\n| A | B |\n|---|---|\n| 1 | **2** |\n\n> a note\n\n```\ncode block\n```\n\n## Second\n\nend\n";
        let g = parse(md);
        assert!(g.unsupported.is_empty(), "{:?}", g.unsupported);
        assert_eq!(g.title, "Title");
        assert_eq!(g.pages.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["About this guide", "First page", "Second"]);
        let p = &g.pages[1];
        assert_eq!(p.subs, vec![("Sub heading".to_string(), "sub-heading".to_string())]);
        assert!(matches!(&p.blocks[2], Block::ListItem { nest: 0, number: None, .. }));
        match &p.blocks[3] {
            Block::ListItem { text, .. } => assert_eq!(plain(text), "b continued"),
            b => panic!("{b:?}"),
        }
        assert!(matches!(&p.blocks[4], Block::ListItem { nest: 1, .. }));
        assert!(matches!(&p.blocks[5], Block::ListItem { number: Some(1), .. }));
        let table = p.blocks.iter().find_map(|b| if let Block::Table { header, rows } = b { Some((header, rows)) } else { None }).unwrap();
        assert_eq!((table.0.len(), table.1.len()), (2, 1));
        assert!(p.blocks.iter().any(|b| matches!(b, Block::Note(_))) && p.blocks.iter().any(|b| matches!(b, Block::Code(c) if c == "code block")));
        // unsupported constructs are reported
        let g = parse("## x\n\n<div>html</div>\n\n---\n\nplain\n");
        assert!(g.unsupported.iter().any(|b| b.starts_with("html:")) && g.unsupported.iter().any(|b| b.starts_with("rule or setext")), "{:?}", g.unsupported);
    }

    #[test]
    fn slugs_are_github_like() {
        assert_eq!(slug("Sections A and B"), "sections-a-and-b");
        assert_eq!(slug("Keyboard shortcuts"), "keyboard-shortcuts");
        assert_eq!(slug("FAQ and troubleshooting"), "faq-and-troubleshooting");
        assert_eq!(slug("Install & start (1)"), "install--start-1");
    }

    #[test]
    fn guide_uses_supported_markdown() {
        let g = parse(GUIDE_MD);
        assert!(g.unsupported.is_empty(), "the guide uses markdown the Help window cannot draw: {:#?}", g.unsupported);
        assert!(g.pages.len() >= 10, "{} pages", g.pages.len());
        // every page has some text besides its heading
        for p in &g.pages {
            assert!(p.blocks.len() > 1, "page {} is empty", p.title);
        }
        // no two pages or sub-headings share an anchor
        let mut anchors = std::collections::HashSet::new();
        for p in &g.pages {
            assert!(anchors.insert(p.anchor.clone()), "duplicate anchor {}", p.anchor);
            for (_, a) in &p.subs {
                assert!(anchors.insert(a.clone()), "duplicate anchor {a}");
            }
        }
    }

    #[test]
    fn guide_menu_paths_resolve() {
        let g = parse(GUIDE_MD);
        let mut n = 0;
        for i in all_inlines(&g) {
            if let Inline::Bold(t) = i {
                if t.contains(" > ") {
                    n += 1;
                    assert!(commands::by_menu_path(t).is_some(), "the guide names the menu path {t:?}, which is not a command's");
                }
            }
        }
        assert!(n >= 20, "only {n} menu paths in the guide");
        // links inside the guide point at pages that exist
        for i in all_inlines(&g) {
            if let Inline::Link { target, .. } = i {
                if let Some(a) = target.strip_prefix('#') {
                    let mut st = HelpState::default();
                    assert!(st.goto(a), "link to #{a} goes nowhere");
                }
            }
        }
    }

    #[test]
    fn guide_shortcuts_match_commands() {
        let want = shortcuts_markdown();
        assert!(GUIDE_MD.replace("\r\n", "\n").contains(&want), "the guide's shortcut table differs from the command table; it must read exactly:\n{want}");
        // the generated page has the table too
        let page = guide().pages.iter().find(|p| p.title == SHORTCUTS_TITLE).expect("shortcuts page");
        let rows = page.blocks.iter().find_map(|b| if let Block::Table { rows, .. } = b { Some(rows.len()) } else { None }).unwrap();
        assert_eq!(rows, commands::all().iter().filter(|i| !i.keys.is_empty()).count() + help_text::FIXED_KEYS.len());
    }

    #[test]
    fn guide_lists_every_rule() {
        let want = rules_markdown();
        assert!(GUIDE_MD.replace("\r\n", "\n").contains(&want), "the guide's rule table differs from help_text::RULES; it must read exactly:\n{want}");
        let page = guide().pages.iter().find(|p| p.title == RULES_TITLE).expect("rules page");
        let rows = page.blocks.iter().find_map(|b| if let Block::Table { rows, .. } = b { Some(rows.len()) } else { None }).unwrap();
        assert_eq!(rows, help_text::RULES.len());
    }

    /// The text of a guide line as a reader sees it: no list marker, no code spans, links as their text.
    fn plain_line(line: &str) -> String {
        let t = line.trim().trim_start_matches('>').trim_start();
        let t = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).unwrap_or(t);
        let t = match t.split_once(". ") {
            Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
            _ => t,
        };
        let mut out = String::new();
        let mut in_code = false;
        for c in t.chars() {
            if c == '`' {
                in_code = !in_code;
                if !in_code {
                    out.push('X');
                }
                continue;
            }
            if !in_code {
                out.push(c);
            }
        }
        // [text](target) becomes text
        let mut res = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find("](") {
            let Some(open) = rest[..i].rfind('[') else { break };
            let Some(close) = rest[i..].find(')') else { break };
            res.push_str(&rest[..open]);
            res.push_str(&rest[open + 1..i]);
            rest = &rest[i + close + 1..];
        }
        res.push_str(rest);
        res.replace("**", "")
    }

    /// Splits a line into sentences: a full stop, ! or ? followed by a space and a capital letter.
    fn guide_sentences(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut cur = String::new();
        for (i, c) in chars.iter().enumerate() {
            cur.push(*c);
            if matches!(c, '.' | '!' | '?') && chars.get(i + 1) == Some(&' ') && chars.get(i + 2).is_some_and(|n| n.is_uppercase()) && !cur.ends_with("...") {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.trim().is_empty() {
            out.push(cur);
        }
        out
    }

    /// The guide's sentences have at most 30 words (tables, code and headings excluded), and the guide says facts as their own
    /// sentences: no ", not " contrast and no "because".
    #[test]
    fn guide_sentences_are_short() {
        let md = GUIDE_MD.replace("\r\n", "\n");
        let mut bad: Vec<String> = Vec::new();
        let mut fenced = false;
        for (n, line) in md.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            let t = line.trim();
            if fenced || t.is_empty() || t.starts_with('|') || t.starts_with('#') || t.starts_with("![") {
                continue;
            }
            let plain = plain_line(t);
            for s in guide_sentences(&plain) {
                let words = s.split_whitespace().count();
                if words > 30 {
                    bad.push(format!("line {}: {words} words: {}", n + 1, s.trim()));
                }
            }
            let lower = plain.to_lowercase();
            for banned in [", not ", "because"] {
                if lower.contains(banned) {
                    bad.push(format!("line {}: has \"{banned}\": {}", n + 1, plain.trim()));
                }
            }
        }
        assert!(bad.is_empty(), "{} guide sentences break the text style:\n{}", bad.len(), bad.join("\n"));
    }

    /// Every "workflow N" in the guide names a workflow that exists, and the words around it fit its title (fix, crash).
    #[test]
    fn guide_workflow_refs_resolve() {
        let md = GUIDE_MD.replace("\r\n", "\n");
        let titles: Vec<(u32, String)> = md
            .lines()
            .filter_map(|l| {
                let (n, t) = l.strip_prefix("### ")?.split_once(". ")?;
                Some((n.parse().ok()?, t.to_string()))
            })
            .collect();
        assert!(titles.len() >= 10, "{titles:?}");
        let mut refs = 0;
        for (n, line) in md.lines().enumerate() {
            if line.starts_with('#') {
                continue;
            }
            let lower = line.to_lowercase();
            let mut from = 0;
            while let Some(i) = lower[from..].find("workflow ") {
                let at = from + i + "workflow ".len();
                from = at;
                let digits: String = lower[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
                let Ok(num) = digits.parse::<u32>() else { continue };
                refs += 1;
                let Some((_, title)) = titles.iter().find(|(k, _)| *k == num) else { panic!("line {}: workflow {num} does not exist", n + 1) };
                let title = title.to_lowercase();
                if lower.contains("fix") {
                    assert!(title.contains("fix"), "line {}: the text says fix, workflow {num} is \"{title}\"", n + 1);
                }
                if lower.contains("crash") {
                    assert!(title.contains("crash"), "line {}: the text says crash, workflow {num} is \"{title}\"", n + 1);
                }
            }
        }
        assert!(refs >= 4, "only {refs} workflow references");
    }

    #[test]
    fn f1_anchors_exist_in_the_guide() {
        let mut st = HelpState::default();
        for tool in Tool::ALL {
            for draw in DrawTarget::ALL {
                let a = anchor_for(tool, draw, None);
                assert!(st.goto(a), "F1 for {tool:?}/{draw:?} wants #{a}, which the guide lacks");
            }
        }
        for s in [SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions, SectionId::SectionA, SectionId::SectionB, SectionId::Blocked, SectionId::Size] {
            let a = anchor_for(Tool::Select, DrawTarget::MobArea, Some(s));
            assert!(st.goto(a), "F1 for {s:?} wants #{a}");
        }
        assert!(st.goto("workflows") && st.goto("getting-started"));
        assert!(st.goto_named("Workflows") && guide().pages[st.page].title == "Workflows");
        assert!(st.goto_named("0") && st.page == 0);
        assert!(!st.goto_named("no such page"));
    }

    #[test]
    fn the_guide_mentions_what_the_editor_does() {
        let text = GUIDE_MD;
        for needle in [
            "F1", "Ctrl+S", "Ctrl+Z", "Ctrl+C", "Del", "Home", "Tab", "Shift+F", "Esc", "wheel", "buildings and shapes", "trees", "3D object distance", "Alt+click", "Generate", "Sync", "data.sah", "Ctrl+P", "Ctrl+G", "--folder",
            "settings.json", "log.txt",
        ] {
            assert!(text.contains(needle), "the guide does not mention {needle}");
        }
    }

    #[test]
    fn search_finds_pages() {
        let g = guide();
        let hits: Vec<&str> = g.pages.iter().filter(|p| page_matches(p, "portal-far")).map(|p| p.title.as_str()).collect();
        assert!(hits.contains(&RULES_TITLE), "{hits:?}");
        assert!(g.pages.iter().all(|p| page_matches(p, "")));
        assert!(!g.pages.iter().any(|p| page_matches(p, "zzzqqq-no-such-text")));
    }
}
