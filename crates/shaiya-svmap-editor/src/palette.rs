//! The command palette (`Ctrl+Shift+P`), quick-open map (`Ctrl+P`) and Go to (`Ctrl+G`): one popup in three modes. This module has
//! the pure parts (fuzzy matching, ranking, the Go to grammar) and the popup window; the app supplies the rows and does what a
//! pick means.

use std::path::PathBuf;

use egui::{Align2, Key, Modifiers, RichText};
use shaiya_svmap::SectionId;

use crate::commands::{self, Cmd};
use crate::settings::Bookmark;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Commands,
    Maps,
    GoTo,
}

impl Mode {
    fn title(self) -> &'static str {
        match self {
            Mode::Commands => "Commands   (type : for Go to; Ctrl+P finds a map)",
            Mode::Maps => "Open a map by number or name   (type > for commands, : for Go to)",
            Mode::GoTo => "Go to   x z · x y z · cell N · area 12 · npc 7 · portal 3 · map 35 · @bookmark · /bmove x z map",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Mode::Commands => "type a command",
            Mode::Maps => "map number, name or file",
            Mode::GoTo => "1662 1655",
        }
    }
}

pub struct Palette {
    pub mode: Mode,
    pub query: String,
    pub selected: usize,
    first_frame: bool,
}

impl Palette {
    pub fn new(mode: Mode) -> Palette {
        Palette { mode, query: String::new(), selected: 0, first_frame: true }
    }

    /// A leading `>` or `:` switches the mode and is removed.
    pub fn normalise_prefix(&mut self) {
        if let Some(r) = self.query.strip_prefix('>') {
            self.mode = Mode::Commands;
            self.query = r.trim_start().to_string();
            self.selected = 0;
        } else if let Some(r) = self.query.strip_prefix(':') {
            self.mode = Mode::GoTo;
            self.query = r.trim_start().to_string();
            self.selected = 0;
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ matching

/// Subsequence match of `query` in `text` (case-insensitive). A higher score is a better match: a match at the start, at word
/// starts and without gaps scores more. None when the query is not a subsequence.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 100;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    let mut first: Option<usize> = None;
    for (i, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            first.get_or_insert(i);
            let word_start = i == 0 || !t[i - 1].is_alphanumeric();
            if word_start {
                score += 8;
            }
            match last {
                Some(l) if l + 1 == i => score += 5,
                Some(l) => score -= (i - l - 1).min(10) as i32,
                None => {}
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    score -= first.unwrap_or(0).min(20) as i32 / 2;
    Some(score)
}

pub struct CmdRow {
    /// The menu path (empty for commands only on a key and in the palette).
    pub path: &'static str,
    pub cmd: Cmd,
    pub text: String,
    pub keys: String,
    pub reason: Option<&'static str>,
}

/// Commands that match the query, best first (the table's order for an empty query); the disabled ones stay in the list, greyed.
pub fn rank_commands(query: &str, can_run: &dyn Fn(Cmd) -> Result<(), &'static str>) -> Vec<CmdRow> {
    let mut rows: Vec<(i32, usize, CmdRow)> = Vec::new();
    for (n, i) in commands::all().iter().enumerate() {
        let hay = format!("{} {}", i.label, i.menu);
        let Some(score) = fuzzy(query, &hay).or_else(|| fuzzy(query, i.help)) else { continue };
        // a match in the label counts more than one in the help text
        let score = if fuzzy(query, &hay).is_some() { score + 50 } else { score };
        let reason = can_run(i.cmd).err();
        rows.push((score, n, CmdRow { cmd: i.cmd, path: i.menu, text: i.label.to_string(), keys: i.keys.first().map_or(String::new(), |k| k.to_string()), reason }));
    }
    if query.trim().is_empty() {
        rows.sort_by_key(|(_, n, r)| (r.reason.is_some(), *n));
    } else {
        rows.sort_by_key(|(s, n, r)| (r.reason.is_some(), std::cmp::Reverse(*s), *n));
    }
    rows.into_iter().map(|(_, _, r)| r).collect()
}

/// A map the quick-open can offer: from the open folder, an open tab or the recent files.
#[derive(Clone, Debug, Default)]
pub struct MapEntry {
    pub id: Option<u32>,
    pub name: String,
    pub path: Option<PathBuf>,
    pub size: Option<u32>,
    /// Some(true) = dungeon, Some(false) = field.
    pub dungeon: Option<bool>,
    pub open: bool,
    pub modified: bool,
    pub recent: bool,
}

impl MapEntry {
    /// `35  Apulune (Alliance of Light)  F 1024  open • modified`.
    pub fn row_text(&self) -> String {
        let id = self.id.map_or("?".to_string(), |i| i.to_string());
        let kind = match self.dungeon {
            Some(true) => "D",
            Some(false) => "F",
            None => "?",
        };
        let size = self.size.map_or("-".to_string(), |s| s.to_string());
        let mut t = format!("{id:>3}  {}  {kind} {size}", if self.name.is_empty() { "(unnamed)" } else { &self.name });
        if self.open {
            t.push_str("  open");
        }
        if self.modified {
            t.push_str(" • modified");
        }
        t
    }
}

/// Score of one map for a query: exact id 1000, id prefix 900, a word of the name starting with the query 500, any subsequence
/// 100 plus its compactness; 50 more for an open or recent map. None = no match.
pub fn map_score(query: &str, e: &MapEntry) -> Option<i32> {
    let q = query.trim().to_lowercase();
    let bonus = if e.open || e.recent { 50 } else { 0 };
    if q.is_empty() {
        return Some(bonus);
    }
    let file = e.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if let Some(id) = e.id {
        let ids = id.to_string();
        if ids == q {
            return Some(1000 + bonus);
        }
        if q.bytes().all(|b| b.is_ascii_digit()) && ids.starts_with(&q) {
            return Some(900 + bonus);
        }
    }
    let name = e.name.to_lowercase();
    if name.split(|c: char| !c.is_alphanumeric()).any(|w| !w.is_empty() && w.starts_with(&q)) || name.starts_with(&q) {
        return Some(500 + bonus);
    }
    if file.contains(&q) {
        return Some(300 + bonus);
    }
    fuzzy(&q, &name).or_else(|| fuzzy(&q, &file)).map(|s| s + bonus)
}

/// Indices into `entries`, best first (stable: the order of the list breaks ties). An empty query lists open and recent maps first.
pub fn rank_maps(query: &str, entries: &[MapEntry]) -> Vec<usize> {
    let mut v: Vec<(i32, usize)> = entries.iter().enumerate().filter_map(|(i, e)| map_score(query, e).map(|s| (s, i))).collect();
    v.sort_by_key(|(s, i)| (std::cmp::Reverse(*s), *i));
    v.into_iter().map(|(_, i)| i).collect()
}

// ------------------------------------------------------------------------------------------------------------------ go to

#[derive(Clone, Debug, PartialEq)]
pub enum Goto {
    Pos { x: f32, y: Option<f32>, z: f32, map: Option<u32> },
    Cell(u64),
    Record(SectionId, usize),
    Map(u32),
    Bookmark(String),
}

fn section_word(w: &str) -> Option<SectionId> {
    Some(match w {
        "area" | "areas" | "a" | "mob" => SectionId::MobAreas,
        "npc" | "npcs" => SectionId::Npcs,
        "portal" | "portals" | "p" => SectionId::Portals,
        "bind" | "binds" => SectionId::Binds,
        "region" | "regions" | "r" => SectionId::Regions,
        "b" => SectionId::SectionB,
        _ => return None,
    })
}

fn num(s: &str) -> Result<f32, String> {
    s.parse::<f32>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("\"{s}\" is not a number"))
}

fn index(s: &str) -> Result<usize, String> {
    s.parse::<usize>().map_err(|_| format!("\"{s}\" is not a whole number"))
}

/// The Go to grammar (case-insensitive, commas optional): `x z`, `x y z`, `cell N`, `<section> N`, `map N`, `@name`,
/// `/bmove x z [map]`.
pub fn parse_goto(input: &str) -> Result<Goto, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("type a position (x z), cell N, area 12, npc 7, portal 3, map 35, @bookmark or /bmove x z map".to_string());
    }
    if let Some(name) = s.strip_prefix('@') {
        return if name.trim().is_empty() { Err("name a bookmark after @".to_string()) } else { Ok(Goto::Bookmark(name.trim().to_string())) };
    }
    let cleaned = s.replace(',', " ");
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let first = words[0].to_lowercase();
    if first == "/bmove" {
        return match words.len() {
            3 | 4 => {
                let map = if words.len() == 4 { Some(words[3].parse::<u32>().map_err(|_| format!("\"{}\" is not a map number", words[3]))?) } else { None };
                Ok(Goto::Pos { x: num(words[1])?, y: None, z: num(words[2])?, map })
            }
            _ => Err("/bmove needs x z and optionally a map: /bmove 1662.2 1655.3 1".to_string()),
        };
    }
    if words.len() == 2 && first == "cell" {
        return words[1].parse::<u64>().map(Goto::Cell).map_err(|_| format!("\"{}\" is not a cell number", words[1]));
    }
    if words.len() == 2 && first == "map" {
        return words[1].parse::<u32>().map(Goto::Map).map_err(|_| format!("\"{}\" is not a map number", words[1]));
    }
    if words.len() == 2 {
        if let Some(sec) = section_word(&first) {
            return Ok(Goto::Record(sec, index(words[1])?));
        }
    }
    match words.len() {
        2 => Ok(Goto::Pos { x: num(words[0])?, y: None, z: num(words[1])?, map: None }),
        3 => Ok(Goto::Pos { x: num(words[0])?, y: Some(num(words[1])?), z: num(words[2])?, map: None }),
        _ => Err(format!("do not know \"{s}\"; try x z, cell N, area 12, npc 7, portal 3, map 35, @bookmark or /bmove x z map")),
    }
}

/// What Go to needs to know about the open map.
pub struct GotoEnv<'a> {
    pub map_id: Option<u32>,
    pub size: Option<u32>,
    pub count: &'a dyn Fn(SectionId) -> usize,
    pub bookmarks: &'a [Bookmark],
    /// The region name at a position of the open map.
    pub region: &'a dyn Fn(f32, f32) -> Option<String>,
    /// Whether a map is in the open folder or a tab.
    pub has_map: &'a dyn Fn(u32) -> bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    Position { map: Option<u32>, x: f32, y: Option<f32>, z: f32, zoom: Option<f32> },
    Select { section: SectionId, index: usize },
    OpenMap(u32),
}

fn section_name(s: SectionId) -> &'static str {
    match s {
        SectionId::MobAreas => "monster area",
        SectionId::Npcs => "NPC",
        SectionId::Portals => "portal",
        SectionId::Binds => "bind area",
        SectionId::Regions => "region",
        SectionId::SectionB => "section B point",
        _ => "record",
    }
}

/// Checks a parsed Go to against the open map and describes it: the target and the live preview line.
pub fn resolve(g: &Goto, env: &GotoEnv) -> Result<(Resolved, String), String> {
    let on_map = |map: Option<u32>| match map.or(env.map_id) {
        Some(m) => format!(" on map {m}"),
        None => String::new(),
    };
    match g {
        Goto::Pos { x, y, z, map } => {
            // a position on another map cannot be range-checked until that map is open
            let target = map.filter(|m| Some(*m) != env.map_id);
            if let Some(m) = target {
                if !(env.has_map)(m) {
                    return Err(format!("map {m} is not in the open folder"));
                }
            } else {
                let size = env.size.ok_or("no map is open")?;
                if *x < 0.0 || *z < 0.0 || *x >= size as f32 || *z >= size as f32 {
                    return Err(format!("x {x:.1} z {z:.1} is outside the map (0 to {size})"));
                }
            }
            let region = if target.is_none() { (env.region)(*x, *z).map(|r| format!(", region \"{r}\"")).unwrap_or_default() } else { String::new() };
            Ok((Resolved::Position { map: target, x: *x, y: *y, z: *z, zoom: None }, format!("→ x {x:.1} z {z:.1}{}{region}", on_map(*map))))
        }
        Goto::Cell(n) => {
            let size = env.size.ok_or("no map is open")? as u64;
            if *n >= size * size {
                return Err(format!("cell {n} is outside the map ({} cells)", size * size));
            }
            let (x, z) = ((n % size) as f32 + 0.5, (n / size) as f32 + 0.5);
            let region = (env.region)(x, z).map(|r| format!(", region \"{r}\"")).unwrap_or_default();
            Ok((Resolved::Position { map: None, x, y: None, z, zoom: None }, format!("→ cell {n} = x {x:.1} z {z:.1}{}{region}", on_map(None))))
        }
        Goto::Record(sec, i) => {
            if env.map_id.is_none() && env.size.is_none() {
                return Err("no map is open".to_string());
            }
            let n = (env.count)(*sec);
            if *i >= n {
                return Err(format!("this map has {n} {}{}, so #{i} does not exist", section_name(*sec), if n == 1 { "" } else { "s" }));
            }
            Ok((Resolved::Select { section: *sec, index: *i }, format!("→ select {} #{i}{}", section_name(*sec), on_map(None))))
        }
        Goto::Map(m) => {
            if (env.has_map)(*m) {
                Ok((Resolved::OpenMap(*m), format!("→ open map {m}")))
            } else {
                Err(format!("map {m} is not in the open folder"))
            }
        }
        Goto::Bookmark(name) => {
            let lower = name.to_lowercase();
            let b = env.bookmarks.iter().find(|b| b.name.to_lowercase() == lower).or_else(|| env.bookmarks.iter().find(|b| b.name.to_lowercase().starts_with(&lower)));
            match b {
                Some(b) => Ok((Resolved::Position { map: None, x: b.x, y: None, z: b.z, zoom: Some(b.zoom) }, format!("→ bookmark \"{}\" x {:.1} z {:.1}{}", b.name, b.x, b.z, on_map(None)))),
                None => Err(if env.bookmarks.is_empty() { format!("no bookmark \"{name}\" on this map (this map has none yet)") } else { format!("no bookmark \"{name}\"; this map has: {}", env.bookmarks.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")) }),
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ the popup

pub struct Row {
    pub text: String,
    pub right: String,
    pub enabled: bool,
    pub why: Option<String>,
}

pub enum Outcome {
    None,
    Close,
    /// Row `i` was picked (Enter on it, or a click).
    Pick(usize),
    /// Enter in Go to mode.
    Submit,
}

/// The popup, top-centre, 520 px wide, at most 12 rows. In Go to mode `preview` is the line under the field (Ok = what will
/// happen, Err = why not).
pub fn show(ctx: &egui::Context, p: &mut Palette, rows: &[Row], preview: Option<&Result<String, String>>) -> Outcome {
    const MAX_ROWS: usize = 12;
    let mut out = Outcome::None;
    // keys first, so the text field does not see them
    let (down, up, enter, esc, page_down, page_up) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
            i.consume_key(Modifiers::NONE, Key::PageDown),
            i.consume_key(Modifiers::NONE, Key::PageUp),
        )
    });
    if esc {
        return Outcome::Close;
    }
    if !rows.is_empty() {
        if down {
            p.selected = (p.selected + 1) % rows.len();
        }
        if up {
            p.selected = (p.selected + rows.len() - 1) % rows.len();
        }
        if page_down {
            p.selected = (p.selected + MAX_ROWS).min(rows.len() - 1);
        }
        if page_up {
            p.selected = p.selected.saturating_sub(MAX_ROWS);
        }
        p.selected = p.selected.min(rows.len() - 1);
    }
    egui::Area::new(egui::Id::unique("palette")).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, 56.0]).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(520.0);
            ui.label(RichText::new(p.mode.title()).small().weak());
            let r = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text(p.mode.hint()).desired_width(f32::INFINITY));
            if p.first_frame {
                r.request_focus();
                p.first_frame = false;
            }
            let before = (p.mode, p.query.clone());
            p.normalise_prefix();
            if r.changed() || before.0 != p.mode {
                p.selected = 0;
            }
            if p.mode == Mode::GoTo {
                if let Some(pre) = preview {
                    match pre {
                        Ok(t) => ui.label(RichText::new(t).color(crate::theme::pal_ui(ui).ok)),
                        Err(e) => ui.label(RichText::new(e).color(crate::theme::pal_ui(ui).error)),
                    };
                }
                if enter {
                    out = Outcome::Submit;
                }
                return;
            }
            if rows.is_empty() {
                ui.label(RichText::new(if p.query.trim().is_empty() { "nothing to show" } else { "no match" }).weak());
            }
            // keep the selected row inside the window of MAX_ROWS rows
            let first = p.selected.saturating_sub(MAX_ROWS - 1).min(rows.len().saturating_sub(MAX_ROWS));
            let first = if p.selected < first { p.selected } else { first };
            for (n, row) in rows.iter().enumerate().skip(first).take(MAX_ROWS) {
                let sel = n == p.selected;
                let resp = ui.horizontal(|ui| {
                    let text = RichText::new(&row.text).monospace();
                    let text = if row.enabled { text } else { text.weak() };
                    let r = ui.add_enabled(true, egui::Button::selectable(sel, text).min_size(egui::vec2(430.0, 0.0)));
                    ui.label(RichText::new(&row.right).weak().small());
                    r
                });
                let r = resp.inner;
                if let Some(why) = &row.why {
                    r.clone().on_hover_text(why);
                }
                if sel {
                    r.scroll_to_me(None);
                }
                if r.clicked() && row.enabled {
                    out = Outcome::Pick(n);
                }
            }
            if enter && !rows.is_empty() && rows[p.selected].enabled {
                out = Outcome::Pick(p.selected);
            }
        });
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, name: &str) -> MapEntry {
        MapEntry { id: Some(id), name: name.to_string(), size: Some(1024), dungeon: Some(false), ..Default::default() }
    }

    fn names() -> Vec<MapEntry> {
        vec![entry(3, "Dungeon of the Mist"), entry(35, "Apulune (Alliance of Light)"), entry(1, "Erina (Alliance of Light)"), entry(5, "Cantabilian"), entry(350, "Test map"), entry(101, "Duplicate 35")]
    }

    #[test]
    fn palette_ranks_maps() {
        let list = names();
        let r = rank_maps("35", &list);
        assert_eq!(list[r[0]].id, Some(35), "the exact id comes first");
        assert_eq!(list[r[1]].id, Some(350), "then the id prefix");
        assert_eq!(list[r[2]].id, Some(101), "then a name with the number as a word");
        let r = rank_maps("apul", &list);
        assert_eq!(list[r[0]].id, Some(35));
        let r = rank_maps("erina", &list);
        assert_eq!(list[r[0]].id, Some(1));
        // a subsequence finds it too
        let r = rank_maps("cntb", &list);
        assert_eq!(list[r[0]].id, Some(5));
        assert!(rank_maps("zzzz", &list).is_empty());
        // open and recent maps float up for an empty query and on ties
        let mut list = names();
        list[3].recent = true;
        list[2].open = true;
        let r = rank_maps("", &list);
        assert_eq!(&r[..2], &[2, 3]);
        let r = rank_maps("alliance", &list);
        assert_eq!(list[r[0]].id, Some(1), "the open one wins the tie");
    }

    #[test]
    fn map_rows_read_like_the_spec() {
        let mut e = entry(35, "Apulune (Alliance of Light)");
        e.size = Some(1024);
        e.open = true;
        e.modified = true;
        assert_eq!(e.row_text(), " 35  Apulune (Alliance of Light)  F 1024  open • modified");
        let r = MapEntry { path: Some(PathBuf::from("D:\\x\\7.svmap")), ..Default::default() };
        assert!(r.row_text().contains("(unnamed)"));
        // a recent file with no id is found by its file name
        assert!(map_score("7.sv", &r).is_some());
    }

    #[test]
    fn fuzzy_rewards_compact_and_leading_matches() {
        let a = fuzzy("sav", "Save").unwrap();
        let b = fuzzy("sav", "Show all views").unwrap();
        assert!(a > b);
        assert!(fuzzy("xyz", "Save").is_none());
        assert_eq!(fuzzy("", "anything"), Some(0));
    }

    /// Menus and palette read one table: every command is in the palette, found by its label, and carries its menu path.
    #[test]
    fn every_command_in_palette() {
        let ok = |_: Cmd| Ok(());
        let all = rank_commands("", &ok);
        assert_eq!(all.len(), commands::all().len());
        for i in commands::all() {
            let row = all.iter().find(|r| r.cmd == i.cmd).unwrap_or_else(|| panic!("{} is not in the palette", i.id));
            assert_eq!(row.path, i.menu, "{}: the palette row carries the menu path", i.id);
            assert!(rank_commands(i.label, &ok).iter().any(|r| r.cmd == i.cmd), "typing the label of {} finds it", i.id);
        }
        // the three-level paths are searchable by their submenu
        assert!(rank_commands("panels dock", &ok).iter().any(|r| r.cmd == Cmd::ShowDock));
        assert!(rank_commands("export csv", &ok).iter().any(|r| r.cmd == Cmd::ExportNpcsCsv));
    }

    #[test]
    fn commands_are_ranked_and_the_disabled_ones_stay_listed() {
        let can = |c: Cmd| if c == Cmd::Save { Err("no map open") } else { Ok(()) };
        let rows = rank_commands("save", &can);
        assert!(rows.iter().any(|r| r.cmd == Cmd::Save && r.reason == Some("no map open")));
        assert_eq!(rows[0].cmd, Cmd::SaveAs, "enabled commands first");
        let all = rank_commands("", &can);
        assert_eq!(all.len(), commands::all().len());
        assert_eq!(all[0].cmd, Cmd::OpenFile);
        let by_menu = rank_commands("tools sync", &can);
        assert!(by_menu.iter().any(|r| r.cmd == Cmd::SyncFromClient), "the menu path is searched too");
        assert_eq!(rank_commands("palette", &can)[0].keys, "Ctrl+Shift+P");
    }

    #[test]
    fn goto_parses() {
        let p = |s: &str| parse_goto(s);
        assert_eq!(p("1662 1655"), Ok(Goto::Pos { x: 1662.0, y: None, z: 1655.0, map: None }));
        assert_eq!(p("1662.5, 1655.25"), Ok(Goto::Pos { x: 1662.5, y: None, z: 1655.25, map: None }));
        assert_eq!(p("10 20.5 30"), Ok(Goto::Pos { x: 10.0, y: Some(20.5), z: 30.0, map: None }));
        assert_eq!(p("cell 3391102"), Ok(Goto::Cell(3391102)));
        assert_eq!(p("CELL 5"), Ok(Goto::Cell(5)));
        assert_eq!(p("area 12"), Ok(Goto::Record(SectionId::MobAreas, 12)));
        assert_eq!(p("npc 7"), Ok(Goto::Record(SectionId::Npcs, 7)));
        assert_eq!(p("portal 3"), Ok(Goto::Record(SectionId::Portals, 3)));
        assert_eq!(p("bind 0"), Ok(Goto::Record(SectionId::Binds, 0)));
        assert_eq!(p("region 9"), Ok(Goto::Record(SectionId::Regions, 9)));
        assert_eq!(p("b 2"), Ok(Goto::Record(SectionId::SectionB, 2)));
        assert_eq!(p("map 35"), Ok(Goto::Map(35)));
        assert_eq!(p("@gate"), Ok(Goto::Bookmark("gate".into())));
        assert_eq!(p("/bmove 1662.2 1655.3 1"), Ok(Goto::Pos { x: 1662.2, y: None, z: 1655.3, map: Some(1) }));
        assert_eq!(p("/bmove 1 2"), Ok(Goto::Pos { x: 1.0, y: None, z: 2.0, map: None }));
        assert_eq!(p("  /BMOVE 5,6  "), Ok(Goto::Pos { x: 5.0, y: None, z: 6.0, map: None }));
        // rejected
        for bad in ["", "   ", "cell", "cell x", "area", "area -1", "area x", "map", "map 1.5", "@", "/bmove", "/bmove 1", "/bmove a b", "/bmove 1 2 x", "1", "1 2 3 4", "x y", "nan 5", "inf 1"] {
            assert!(p(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    fn env<'a>(count: &'a dyn Fn(SectionId) -> usize, region: &'a dyn Fn(f32, f32) -> Option<String>, has: &'a dyn Fn(u32) -> bool, marks: &'a [Bookmark]) -> GotoEnv<'a> {
        GotoEnv { map_id: Some(1), size: Some(2048), count, bookmarks: marks, region, has_map: has }
    }

    #[test]
    fn goto_resolves_against_the_open_map() {
        let count = |s: SectionId| match s {
            SectionId::Npcs => 286,
            SectionId::MobAreas => 835,
            _ => 0,
        };
        let region = |x: f32, z: f32| if x > 1600.0 && z > 1600.0 { Some("Arktuis Village".to_string()) } else { None };
        let has = |m: u32| m == 1 || m == 35;
        let marks = [Bookmark { name: "Gate".into(), x: 100.0, z: 200.0, zoom: 4.0 }];
        let e = env(&count, &region, &has, &marks);
        let ok = |s: &str| resolve(&parse_goto(s).unwrap(), &e);
        let (r, pre) = ok("1662 1655.5").unwrap();
        assert_eq!(r, Resolved::Position { map: None, x: 1662.0, y: None, z: 1655.5, zoom: None });
        assert_eq!(pre, "→ x 1662.0 z 1655.5 on map 1, region \"Arktuis Village\"");
        let (r, _) = ok("cell 3391102").unwrap();
        assert_eq!(r, Resolved::Position { map: None, x: 1662.5, y: None, z: 1655.5, zoom: None });
        assert_eq!(ok("npc 285").unwrap().0, Resolved::Select { section: SectionId::Npcs, index: 285 });
        assert!(ok("npc 286").unwrap_err().contains("has 286 NPCs"));
        assert!(ok("portal 0").unwrap_err().contains("has 0 portals"));
        assert_eq!(ok("map 35").unwrap().0, Resolved::OpenMap(35));
        assert!(ok("map 36").unwrap_err().contains("not in the open folder"));
        assert!(ok("3000 10").unwrap_err().contains("outside the map"));
        assert!(ok("-1 10").unwrap_err().contains("outside the map"));
        assert!(ok("cell 4194304").unwrap_err().contains("outside the map"));
        assert_eq!(ok("/bmove 10 20 35").unwrap().0, Resolved::Position { map: Some(35), x: 10.0, y: None, z: 20.0, zoom: None });
        assert!(ok("/bmove 10 20 36").is_err());
        assert_eq!(ok("/bmove 10 20 1").unwrap().0, Resolved::Position { map: None, x: 10.0, y: None, z: 20.0, zoom: None });
        // bookmarks: exact, by prefix, unknown
        assert_eq!(ok("@gate").unwrap().0, Resolved::Position { map: None, x: 100.0, z: 200.0, y: None, zoom: Some(4.0) });
        assert!(ok("@ga").is_ok());
        assert!(ok("@castle").unwrap_err().contains("this map has: Gate"));
        let none = GotoEnv { bookmarks: &[], ..env(&count, &region, &has, &[]) };
        assert!(resolve(&parse_goto("@x").unwrap(), &none).unwrap_err().contains("has none yet"));
        // no map open
        let closed = GotoEnv { map_id: None, size: None, ..env(&count, &region, &has, &[]) };
        assert_eq!(resolve(&parse_goto("1 2").unwrap(), &closed).unwrap_err(), "no map is open");
    }

    #[test]
    fn prefixes_switch_the_mode() {
        let mut p = Palette::new(Mode::Maps);
        p.query = ">save".into();
        p.normalise_prefix();
        assert_eq!((p.mode, p.query.as_str()), (Mode::Commands, "save"));
        p.query = ": 10 20".into();
        p.normalise_prefix();
        assert_eq!((p.mode, p.query.as_str()), (Mode::GoTo, "10 20"));
        p.query = "plain".into();
        p.normalise_prefix();
        assert_eq!(p.mode, Mode::GoTo);
    }
}

#[cfg(test)]
mod real_names {
    use super::*;

    /// With the real client: the map names come from `sysmsg-uni.txt` and "erina" / "apul" find maps 1 and 35.
    #[test]
    fn palette_ranks_maps_with_the_real_names() {
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let src = shaiya_clientdata::ClientSource::open(std::path::Path::new(&dir)).unwrap();
        let sys = src.sysmsg().unwrap();
        let ids: Vec<u32> = (0..=67).chain(100..=108).collect();
        let entries: Vec<MapEntry> = ids.iter().map(|id| MapEntry { id: Some(*id), name: shaiya_clientdata::map_name(*id, Some(&sys)), ..Default::default() }).collect();
        let first = |q: &str| entries[rank_maps(q, &entries)[0]].id;
        assert_eq!(first("erina"), Some(1));
        assert_eq!(first("apul"), Some(35));
        assert_eq!(first("35"), Some(35));
        assert_eq!(first("proelium"), Some(18));
    }
}
