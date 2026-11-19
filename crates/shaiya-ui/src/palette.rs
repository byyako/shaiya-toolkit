//! The command palette (Ctrl+Shift+P): a fuzzy list of commands. Copied from the svmap editor's `palette.rs` and reduced to commands; the
//! shell supplies its own commands plus the tool's.

use egui::{Align2, Key, Modifiers, RichText};

/// Subsequence match of `query` in `text` (case-insensitive). A higher score is a better match: a match at the start, at word starts
/// and without gaps scores more. `None` when the query is not a subsequence.
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

/// Indices of `labels` that match `query`, best first (stable on ties, so the table order decides).
pub fn rank(query: &str, labels: &[String]) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = labels.iter().enumerate().filter_map(|(i, l)| fuzzy(query, l).map(|s| (s, i))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[derive(Default)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
    pub first_frame: bool,
}

impl Palette {
    pub fn new() -> Palette {
        Palette { query: String::new(), selected: 0, first_frame: true }
    }
}

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
}

/// The popup, top-centre, 520 px wide, at most 12 rows.
pub fn show(ctx: &egui::Context, p: &mut Palette, rows: &[Row]) -> Outcome {
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
    let first_frame = p.first_frame;
    let area = egui::Area::new(egui::Id::unique("shaiya-palette")).order(egui::Order::Foreground).anchor(Align2::CENTER_TOP, [0.0, 56.0]).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(520.0);
            ui.label(RichText::new("Command palette").small().weak());
            let r = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text("Type a command").desired_width(f32::INFINITY));
            if p.first_frame {
                r.request_focus();
                p.first_frame = false;
            }
            if r.changed() {
                p.selected = 0;
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
                    let text = RichText::new(&row.text);
                    let text = if row.enabled { text } else { text.weak() };
                    let r = ui.add(egui::Button::selectable(sel, text).min_size(egui::vec2(430.0, 0.0)));
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
    // a primary press outside the popup closes it (not on its first frame, and a pick wins)
    if matches!(out, Outcome::None) && !first_frame {
        let outside = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|pos| !area.response.rect.contains(pos)));
        if outside {
            return Outcome::Close;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_starts_words_and_runs() {
        assert!(fuzzy("zzz", "Open file").is_none());
        assert_eq!(fuzzy("", "anything"), Some(0));
        let a = fuzzy("open", "Open file...").unwrap();
        let b = fuzzy("open", "Reopen the thing").unwrap();
        assert!(a > b, "a match at the start beats one inside a word");
    }

    #[test]
    fn rank_orders_by_score_then_position() {
        let labels: Vec<String> = ["Open file...", "Open path...", "Preferences...", "Quit", "Zoom in", "Zoom out", "Zoom reset"].iter().map(|s| s.to_string()).collect();
        let r = rank("zoom", &labels);
        assert_eq!(r, vec![4, 5, 6]);
        let r = rank("op", &labels);
        assert_eq!(&r[..2], &[0, 1]);
        assert!(rank("qzx", &labels).is_empty());
        assert_eq!(rank("", &labels).len(), labels.len());
        assert_eq!(rank("pref", &labels)[0], 2);
    }
}
