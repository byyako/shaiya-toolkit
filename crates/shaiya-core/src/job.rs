//! Progress reporting and cancellation for long work. Library code takes a `&Job`; the CLI passes a terminal sink, the GUI a task sink.

use crate::error::{Error, Result};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Bytes,
    Files,
    Records,
    Items,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// Receives progress. No Send/Sync bound: a sink is used on the thread that runs the work.
pub trait Progress {
    fn stage(&self, _name: &str) {}
    fn total(&self, _total: Option<u64>, _unit: Unit) {}
    fn advance(&self, _n: u64) {}
    fn note(&self, _level: Level, _text: &str) {}
}

/// A sink that does nothing.
pub struct Silent;

impl Progress for Silent {}

/// Calls `f(done, total)` on every advance (and on every `total`, with the count reset to 0).
pub struct FnProgress<F: Fn(u64, Option<u64>)> {
    f: F,
    done: Cell<u64>,
    total: Cell<Option<u64>>,
}

impl<F: Fn(u64, Option<u64>)> FnProgress<F> {
    pub fn new(f: F) -> Self {
        FnProgress { f, done: Cell::new(0), total: Cell::new(None) }
    }
}

impl<F: Fn(u64, Option<u64>)> Progress for FnProgress<F> {
    fn total(&self, total: Option<u64>, _unit: Unit) {
        self.total.set(total);
        self.done.set(0);
        (self.f)(0, total);
    }
    fn advance(&self, n: u64) {
        let d = self.done.get().saturating_add(n);
        self.done.set(d);
        (self.f)(d, self.total.get());
    }
}

static SILENT: Silent = Silent;

pub struct Job<'a> {
    progress: &'a dyn Progress,
    cancel: Cancel,
}

impl<'a> Job<'a> {
    pub fn new(progress: &'a dyn Progress, cancel: Cancel) -> Job<'a> {
        Job { progress, cancel }
    }
    pub fn silent() -> Job<'static> {
        Job { progress: &SILENT, cancel: Cancel::new() }
    }
    pub fn stage(&self, name: &str) {
        self.progress.stage(name);
    }
    pub fn total(&self, total: Option<u64>, unit: Unit) {
        self.progress.total(total, unit);
    }
    pub fn advance(&self, n: u64) {
        self.progress.advance(n);
    }
    pub fn info(&self, text: &str) {
        self.progress.note(Level::Info, text);
    }
    pub fn warn(&self, text: &str) {
        self.progress.note(Level::Warn, text);
    }
    /// Err(Error::Cancelled) once cancelled. Long loops call it at least every 64 MiB of I/O or 250 ms of work.
    pub fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn cancel_token(&self) -> &Cancel {
        &self.cancel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn check_fails_after_cancel() {
        let c = Cancel::new();
        let j = Job::new(&Silent, c.clone());
        assert!(j.check().is_ok());
        c.cancel();
        assert!(c.is_cancelled());
        assert!(matches!(j.check(), Err(Error::Cancelled)));
        assert!(j.cancel_token().is_cancelled());
        assert!(Job::silent().check().is_ok());
    }

    #[test]
    fn fn_progress_counts() {
        let seen = RefCell::new(Vec::new());
        let p = FnProgress::new(|d, t| seen.borrow_mut().push((d, t)));
        let job = Job::new(&p, Cancel::new());
        job.total(Some(10), Unit::Bytes);
        job.advance(4);
        job.advance(6);
        assert_eq!(*seen.borrow(), vec![(0, Some(10)), (4, Some(10)), (10, Some(10))]);
    }
}
