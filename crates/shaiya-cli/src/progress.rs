//! The terminal progress sink: `[stage] 45%  900 MB / 2.0 GB  120 MB/s  ETA 8 s` on stderr, at most 10 updates per second.

use shaiya_core::time::human_size;
use shaiya_core::{Level, Progress, Unit};
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Only when stderr is a terminal.
    Auto,
    /// A line per 10% (for logs).
    Always,
    Never,
}

pub struct TermProgress {
    quiet: bool,
    live: bool,
    lines: bool,
    stage: RefCell<String>,
    unit: Cell<Unit>,
    total: Cell<Option<u64>>,
    done: Cell<u64>,
    started: Cell<Instant>,
    last_draw: Cell<Option<Instant>>,
    last_decile: Cell<u64>,
    drawn: Cell<bool>,
}

impl TermProgress {
    /// `stderr_is_terminal`: whether stderr is a terminal (from `std::io::IsTerminal`).
    pub fn new(mode: Mode, quiet: bool, stderr_is_terminal: bool) -> TermProgress {
        let (live, lines) = match mode {
            _ if quiet => (false, false),
            Mode::Never => (false, false),
            Mode::Auto => (stderr_is_terminal, false),
            Mode::Always => (stderr_is_terminal, !stderr_is_terminal),
        };
        TermProgress {
            quiet,
            live,
            lines,
            stage: RefCell::new(String::new()),
            unit: Cell::new(Unit::Bytes),
            total: Cell::new(None),
            done: Cell::new(0),
            started: Cell::new(Instant::now()),
            last_draw: Cell::new(None),
            last_decile: Cell::new(0),
            drawn: Cell::new(false),
        }
    }

    fn amount(&self, n: u64) -> String {
        match self.unit.get() {
            Unit::Bytes => human_size(n),
            Unit::Files => format!("{n} files"),
            Unit::Records => format!("{n} records"),
            Unit::Items => format!("{n} items"),
        }
    }

    fn render(&self) -> String {
        let (done, total) = (self.done.get(), self.total.get());
        let secs = self.started.get().elapsed().as_secs_f64().max(0.001);
        let rate = done as f64 / secs;
        let mut s = format!("[{}]", self.stage.borrow());
        match total {
            Some(t) if t > 0 => {
                let pct = (done.min(t) * 100 / t) as u32;
                s.push_str(&format!("  {pct}%  {} / {}", self.amount(done), self.amount(t)));
                if self.unit.get() == Unit::Bytes {
                    s.push_str(&format!("  {}/s", human_size(rate as u64)));
                }
                if rate > 0.0 && done < t {
                    s.push_str(&format!("  ETA {} s", ((t - done) as f64 / rate).ceil() as u64));
                }
            }
            _ => s.push_str(&format!("  {}", self.amount(done))),
        }
        s
    }

    fn draw(&self, force: bool) {
        let now = Instant::now();
        if self.live {
            if !force && self.last_draw.get().is_some_and(|t| now.duration_since(t) < Duration::from_millis(100)) {
                return;
            }
            self.last_draw.set(Some(now));
            let mut e = std::io::stderr().lock();
            let _ = write!(e, "\r{:<79}", self.render());
            let _ = e.flush();
            self.drawn.set(true);
        } else if self.lines {
            let Some(t) = self.total.get().filter(|&t| t > 0) else { return };
            let decile = (self.done.get().min(t) * 10 / t) as u64;
            if decile > self.last_decile.get() {
                self.last_decile.set(decile);
                eprintln!("{}", self.render());
            }
        }
    }

    fn clear_line(&self) {
        if self.live && self.drawn.replace(false) {
            let mut e = std::io::stderr().lock();
            let _ = write!(e, "\r{:<79}\r", "");
            let _ = e.flush();
        }
    }

    /// Ends the progress line (call when a long step is over).
    pub fn finish(&self) {
        self.clear_line();
    }
}

impl Progress for TermProgress {
    fn stage(&self, name: &str) {
        self.clear_line();
        *self.stage.borrow_mut() = name.to_string();
    }
    fn total(&self, total: Option<u64>, unit: Unit) {
        self.clear_line();
        self.total.set(total);
        self.unit.set(unit);
        self.done.set(0);
        self.started.set(Instant::now());
        self.last_decile.set(0);
        self.last_draw.set(None);
    }
    fn advance(&self, n: u64) {
        self.done.set(self.done.get().saturating_add(n));
        self.draw(false);
    }
    fn note(&self, level: Level, text: &str) {
        if self.quiet {
            return;
        }
        self.clear_line();
        match level {
            Level::Info => eprintln!("{text}"),
            Level::Warn => eprintln!("warning: {text}"),
            Level::Error => eprintln!("error: {text}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_shows_percent_sizes_and_stage() {
        let p = TermProgress::new(Mode::Never, false, false);
        p.stage("hash");
        p.total(Some(2_000_000), Unit::Bytes);
        p.done.set(900_000);
        let s = p.render();
        assert!(s.starts_with("[hash]  45%  900.0 KB / 2.0 MB"), "{s}");
        p.total(None, Unit::Files);
        p.done.set(3);
        assert_eq!(p.render(), "[hash]  3 files");
    }

    #[test]
    fn mode_rules() {
        assert!(TermProgress::new(Mode::Auto, false, true).live);
        assert!(!TermProgress::new(Mode::Auto, false, false).live);
        assert!(TermProgress::new(Mode::Always, false, false).lines);
        assert!(!TermProgress::new(Mode::Never, false, true).live);
        assert!(!TermProgress::new(Mode::Always, true, true).live, "quiet wins");
    }
}
