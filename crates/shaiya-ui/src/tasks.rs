//! Background tasks. File work never runs on the UI thread: a tool spawns it through [`Tasks::spawn`], which gives it a `shaiya_core::Job`
//! (progress and cancel), catches panics, serialises tasks that share an `exclusive` key, and shows every task in the Activity panel.

use shaiya_core::{Cancel, Error, Job, Level, Progress, Unit};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub struct TaskSpec {
    pub title: String,
    pub cancellable: bool,
    /// Tasks with the same key run one at a time.
    pub exclusive: Option<String>,
}

impl TaskSpec {
    pub fn new(title: impl Into<String>) -> TaskSpec {
        TaskSpec { title: title.into(), cancellable: true, exclusive: None }
    }
}

pub enum TaskOutcome<T> {
    Done(T),
    Failed(Error),
    Cancelled,
    Panicked(String),
}

/// How a finished task ended, as the Activity panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finish {
    Done,
    Failed(String),
    Cancelled,
    Panicked(String),
}

/// A line of the Activity log.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub at: SystemTime,
    pub level: Level,
    pub text: String,
}

/// The state of one task, shared between the worker and the UI.
pub struct TaskState {
    pub id: u64,
    pub title: String,
    pub cancellable: bool,
    pub cancel: Cancel,
    pub started: Instant,
    pub done: AtomicU64,
    inner: Mutex<StateInner>,
}

#[derive(Default)]
struct StateInner {
    stage: String,
    total: Option<u64>,
    unit: Option<Unit>,
    waiting: bool,
    finished: Option<(Finish, Duration)>,
}

/// A copy of the displayable parts of a task.
#[derive(Clone, Debug)]
pub struct TaskView {
    pub id: u64,
    pub title: String,
    pub cancellable: bool,
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
    pub unit: Unit,
    pub waiting: bool,
    pub elapsed: Duration,
    pub finished: Option<Finish>,
}

impl TaskView {
    /// 0.0 to 1.0 when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        self.total.filter(|&t| t > 0).map(|t| (self.done.min(t) as f32 / t as f32).clamp(0.0, 1.0))
    }
}

impl TaskState {
    pub fn view(&self) -> TaskView {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        TaskView {
            id: self.id,
            title: self.title.clone(),
            cancellable: self.cancellable,
            stage: g.stage.clone(),
            done: self.done.load(Ordering::Relaxed),
            total: g.total,
            unit: g.unit.unwrap_or(Unit::Bytes),
            waiting: g.waiting,
            elapsed: g.finished.as_ref().map_or_else(|| self.started.elapsed(), |f| f.1),
            finished: g.finished.as_ref().map(|f| f.0.clone()),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).finished.is_some()
    }
}

/// The progress sink a worker thread uses; it writes into the shared task state and the Activity log.
struct TaskSink {
    state: Arc<TaskState>,
    log: Arc<Mutex<Vec<LogLine>>>,
    #[cfg(any(test, feature = "testing"))]
    hold: Arc<Hold>,
}

/// Test hook (feature `testing`): while set, a task's progress reports stop at the first `advance` until the hold is released or the
/// task is cancelled, so a test can cancel a task at a point it cannot have passed.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
struct Hold {
    on: Mutex<bool>,
    cv: Condvar,
}

#[cfg(any(test, feature = "testing"))]
impl Hold {
    fn wait(&self, cancel: &Cancel) {
        let mut on = self.on.lock().unwrap_or_else(|p| p.into_inner());
        while *on && !cancel.is_cancelled() {
            on = self.cv.wait_timeout(on, Duration::from_millis(20)).unwrap_or_else(|p| p.into_inner()).0;
        }
    }
}

impl Progress for TaskSink {
    fn stage(&self, name: &str) {
        self.state.inner.lock().unwrap_or_else(|p| p.into_inner()).stage = name.to_string();
    }
    fn total(&self, total: Option<u64>, unit: Unit) {
        let mut g = self.state.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.total = total;
        g.unit = Some(unit);
        self.state.done.store(0, Ordering::Relaxed);
    }
    fn advance(&self, n: u64) {
        self.state.done.fetch_add(n, Ordering::Relaxed);
        #[cfg(any(test, feature = "testing"))]
        self.hold.wait(&self.state.cancel);
    }
    fn note(&self, level: Level, text: &str) {
        self.log.lock().unwrap_or_else(|p| p.into_inner()).push(LogLine { at: SystemTime::now(), level, text: text.to_string() });
    }
}

pub struct TaskHandle<T> {
    id: u64,
    rx: Receiver<TaskOutcome<T>>,
    cancel: Cancel,
    state: Arc<TaskState>,
    finished: bool,
}

impl<T> TaskHandle<T> {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The outcome once the task has ended (once; later calls give `None`).
    pub fn poll(&mut self) -> Option<TaskOutcome<T>> {
        if self.finished {
            return None;
        }
        match self.rx.try_recv() {
            Ok(o) => {
                self.finished = true;
                Some(o)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.finished = true;
                Some(TaskOutcome::Panicked("the worker thread ended without a result".into()))
            }
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// True once the worker has ended (the outcome may still be waiting in `poll`).
    pub fn is_finished(&self) -> bool {
        self.state.is_finished()
    }
}

/// Keys of exclusive tasks that are running, with the condition variable waiting workers sleep on.
#[derive(Default)]
struct Gate {
    held: Mutex<HashSet<String>>,
    cv: Condvar,
}

pub struct Tasks {
    next_id: u64,
    states: Vec<Arc<TaskState>>,
    log: Arc<Mutex<Vec<LogLine>>>,
    gate: Arc<Gate>,
    ctx: Option<egui::Context>,
    #[cfg(any(test, feature = "testing"))]
    hold: Arc<Hold>,
}

/// Finished tasks stay in the list (for the Activity panel) up to this many.
const KEEP_FINISHED: usize = 20;

impl Tasks {
    pub fn new(ctx: Option<egui::Context>) -> Tasks {
        Tasks { next_id: 1, states: Vec::new(), log: Arc::new(Mutex::new(Vec::new())), gate: Arc::new(Gate::default()), ctx, #[cfg(any(test, feature = "testing"))] hold: Arc::new(Hold::default()) }
    }

    /// Test hook: from now on every task stops at its first progress report until [`Tasks::release_progress`] or until it is cancelled.
    #[cfg(any(test, feature = "testing"))]
    pub fn hold_progress(&self) {
        *self.hold.on.lock().unwrap_or_else(|p| p.into_inner()) = true;
    }

    /// Test hook: lets tasks held by [`Tasks::hold_progress`] go on.
    #[cfg(any(test, feature = "testing"))]
    pub fn release_progress(&self) {
        *self.hold.on.lock().unwrap_or_else(|p| p.into_inner()) = false;
        self.hold.cv.notify_all();
    }

    /// Runs `work` on a worker thread. A panic becomes `TaskOutcome::Panicked`.
    pub fn spawn<T, F>(&mut self, spec: TaskSpec, work: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&Job) -> shaiya_core::Result<T> + Send + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        let cancel = Cancel::new();
        let state = Arc::new(TaskState {
            id,
            title: spec.title.clone(),
            cancellable: spec.cancellable,
            cancel: cancel.clone(),
            started: Instant::now(),
            done: AtomicU64::new(0),
            inner: Mutex::new(StateInner::default()),
        });
        // drop the oldest finished tasks beyond the cap
        let finished: Vec<usize> = self.states.iter().enumerate().filter(|(_, s)| s.is_finished()).map(|(i, _)| i).collect();
        if finished.len() > KEEP_FINISHED {
            for i in finished[..finished.len() - KEEP_FINISHED].iter().rev() {
                self.states.remove(*i);
            }
        }
        self.states.push(state.clone());
        let handle_state = state.clone();
        let (tx, rx) = channel();
        let (log, gate, ctx) = (self.log.clone(), self.gate.clone(), self.ctx.clone());
        #[cfg(any(test, feature = "testing"))]
        let hold = self.hold.clone();
        let key = spec.exclusive.clone();
        let handle_cancel = cancel.clone();
        let spawned = std::thread::Builder::new().name(format!("task-{id}")).spawn(move || {
            let sink = TaskSink { state: state.clone(), log: log.clone(), #[cfg(any(test, feature = "testing"))] hold };
            // wait for the exclusive key
            let mut cancelled_waiting = false;
            if let Some(k) = &key {
                let mut held = gate.held.lock().unwrap_or_else(|p| p.into_inner());
                while held.contains(k) {
                    state.inner.lock().unwrap_or_else(|p| p.into_inner()).waiting = true;
                    if cancel.is_cancelled() {
                        cancelled_waiting = true;
                        break;
                    }
                    held = gate.cv.wait_timeout(held, Duration::from_millis(100)).unwrap_or_else(|p| p.into_inner()).0;
                }
                // cancelled while waiting for the key: it never starts, and never takes the key
                if !cancelled_waiting && cancel.is_cancelled() {
                    cancelled_waiting = true;
                }
                if !cancelled_waiting {
                    held.insert(k.clone());
                }
                state.inner.lock().unwrap_or_else(|p| p.into_inner()).waiting = false;
            }
            let outcome = if cancelled_waiting {
                TaskOutcome::Cancelled
            } else {
                let job = Job::new(&sink, cancel.clone());
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&job)));
                match result {
                    Ok(Ok(v)) => TaskOutcome::Done(v),
                    Ok(Err(Error::Cancelled)) => TaskOutcome::Cancelled,
                    Ok(Err(e)) => TaskOutcome::Failed(e),
                    Err(p) => {
                        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
                        TaskOutcome::Panicked(msg)
                    }
                }
            };
            if let Some(k) = &key {
                if !cancelled_waiting {
                    gate.held.lock().unwrap_or_else(|p| p.into_inner()).remove(k);
                    gate.cv.notify_all();
                }
            }
            let finish = match &outcome {
                TaskOutcome::Done(_) => Finish::Done,
                TaskOutcome::Failed(e) => Finish::Failed(e.to_string()),
                TaskOutcome::Cancelled => Finish::Cancelled,
                TaskOutcome::Panicked(m) => Finish::Panicked(m.clone()),
            };
            let (level, text) = match &finish {
                Finish::Done => (Level::Info, format!("{}: done", state.title)),
                Finish::Failed(e) => (Level::Error, format!("{}: {e}", state.title)),
                Finish::Cancelled => (Level::Warn, format!("{}: cancelled", state.title)),
                Finish::Panicked(m) => (Level::Error, format!("{}: internal error (a bug): {m}", state.title)),
            };
            log.lock().unwrap_or_else(|p| p.into_inner()).push(LogLine { at: SystemTime::now(), level, text });
            state.inner.lock().unwrap_or_else(|p| p.into_inner()).finished = Some((finish, state.started.elapsed()));
            let _ = tx.send(outcome);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        if let Err(e) = spawned {
            // no thread could be started: report it as a failed task through a closed channel
            self.log.lock().unwrap_or_else(|p| p.into_inner()).push(LogLine { at: SystemTime::now(), level: Level::Error, text: format!("cannot start a worker thread: {e}") });
        }
        TaskHandle { id, rx, cancel: handle_cancel, state: handle_state, finished: false }
    }

    /// Tasks that have not ended.
    pub fn running(&self) -> usize {
        self.states.iter().filter(|s| !s.is_finished()).count()
    }

    pub fn cancel_all(&self) {
        for s in &self.states {
            if !s.is_finished() {
                s.cancel.cancel();
            }
        }
    }

    /// Cancel one task by id.
    pub fn cancel(&self, id: u64) {
        if let Some(s) = self.states.iter().find(|s| s.id == id) {
            s.cancel.cancel();
        }
    }

    /// Views of every task still in the list, oldest first.
    pub fn views(&self) -> Vec<TaskView> {
        self.states.iter().map(|s| s.view()).collect()
    }

    /// Drains the lines tasks and tools added to the Activity log since the last call.
    pub fn take_log(&self) -> Vec<LogLine> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|p| p.into_inner()))
    }

    pub fn log_handle(&self) -> Arc<Mutex<Vec<LogLine>>> {
        self.log.clone()
    }

    /// Waits until every running task has ended, at most `per_task` for each of them. Returns how many were still running.
    pub fn wait_all(&self, per_task: Duration) -> usize {
        let running: Vec<&Arc<TaskState>> = self.states.iter().filter(|s| !s.is_finished()).collect();
        let deadline = Instant::now() + per_task * running.len() as u32;
        while Instant::now() < deadline && running.iter().any(|s| !s.is_finished()) {
            std::thread::sleep(Duration::from_millis(10));
        }
        running.iter().filter(|s| !s.is_finished()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait<T>(h: &mut TaskHandle<T>) -> TaskOutcome<T> {
        for _ in 0..500 {
            if let Some(o) = h.poll() {
                return o;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("task did not finish");
    }

    #[test]
    fn a_task_reports_progress_and_a_result() {
        let mut t = Tasks::new(None);
        let mut h = t.spawn(TaskSpec::new("count"), |job| {
            job.stage("counting");
            job.total(Some(10), Unit::Items);
            for _ in 0..10 {
                job.advance(1);
            }
            job.info("counted");
            Ok(42)
        });
        assert!(matches!(wait(&mut h), TaskOutcome::Done(42)));
        assert!(h.poll().is_none(), "the outcome is delivered once");
        let v = t.views();
        assert_eq!((v[0].stage.as_str(), v[0].done, v[0].total, v[0].finished.clone()), ("counting", 10, Some(10), Some(Finish::Done)));
        assert_eq!(t.running(), 0);
        let log = t.take_log();
        assert!(log.iter().any(|l| l.text == "counted") && log.iter().any(|l| l.text == "count: done"));
        assert!(t.take_log().is_empty());
    }

    #[test]
    fn errors_cancel_and_panics_are_outcomes() {
        let mut t = Tasks::new(None);
        let mut h = t.spawn(TaskSpec::new("fails"), |_| -> shaiya_core::Result<()> { Err(Error::Refused("no".into())) });
        assert!(matches!(wait(&mut h), TaskOutcome::Failed(Error::Refused(_))));
        let mut h = t.spawn(TaskSpec::new("cancels"), |job| {
            loop {
                job.check()?;
                std::thread::sleep(Duration::from_millis(5));
            }
            #[allow(unreachable_code)]
            Ok(())
        });
        h.cancel();
        assert!(matches!(wait(&mut h), TaskOutcome::Cancelled));
        let mut h = t.spawn(TaskSpec::new("panics"), |_| -> shaiya_core::Result<()> { panic!("boom") });
        match wait(&mut h) {
            TaskOutcome::Panicked(m) => assert!(m.contains("boom")),
            _ => panic!("expected a panic outcome"),
        }
        assert!(t.views().iter().any(|v| matches!(&v.finished, Some(Finish::Panicked(_)))));
    }

    #[test]
    fn exclusive_tasks_run_one_at_a_time() {
        let mut t = Tasks::new(None);
        let active = Arc::new(AtomicU64::new(0));
        let max_seen = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        for i in 0..3 {
            let (a, m) = (active.clone(), max_seen.clone());
            let spec = TaskSpec { title: format!("t{i}"), cancellable: true, exclusive: Some("file".into()) };
            handles.push(t.spawn(spec, move |_| {
                let now = a.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                a.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            }));
        }
        for h in &mut handles {
            assert!(matches!(wait(h), TaskOutcome::Done(())));
        }
        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
        // different keys may overlap, and cancel_all stops a waiting task
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let mut a = t.spawn(TaskSpec { title: "hold".into(), cancellable: true, exclusive: Some("k".into()) }, move |job| -> shaiya_core::Result<()> {
            let _ = held_tx.send(());
            while job.check().is_ok() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(Error::Cancelled)
        });
        // the first task must hold the key before the second is spawned (thread start order is not guaranteed)
        held_rx.recv_timeout(Duration::from_secs(30)).expect("the first task took the key");
        let mut b = t.spawn(TaskSpec { title: "wait".into(), cancellable: true, exclusive: Some("k".into()) }, |_| Ok(()));
        // the second task must show up as waiting; it cannot finish while the first holds the key
        let deadline = Instant::now() + Duration::from_secs(30);
        while !t.views().iter().any(|v| v.waiting) {
            assert!(Instant::now() < deadline, "the second task waits for the key");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(t.running(), 2);
        t.cancel_all();
        assert!(matches!(wait(&mut a), TaskOutcome::Cancelled));
        assert!(matches!(wait(&mut b), TaskOutcome::Cancelled));
        assert_eq!(t.wait_all(Duration::from_secs(1)), 0);
    }
}
