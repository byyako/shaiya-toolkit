//! Undo and redo, generic over the edit type (no egui in here). Generalised from the svmap editor's `doc.rs` (`Step`, a 1500 ms merge
//! window, at most 5000 steps, the saved depth that tells whether the document is dirty).

use std::time::{Duration, Instant};

pub const MERGE_WINDOW: Duration = Duration::from_millis(1500);
pub const MAX_STEPS: usize = 5000;

/// One reversible change to a model `M`.
pub trait Edit<M> {
    fn apply(&self, model: &mut M);
    /// The edit that undoes this one.
    fn invert(&self) -> Self
    where
        Self: Sized;
}

struct Step<E> {
    label: String,
    edits: Vec<E>,
    key: Option<String>,
    at: Instant,
}

pub struct History<E> {
    undo: Vec<Step<E>>,
    redo: Vec<Step<E>>,
    /// The undo depth at which the document equals what was saved (`None` when that state is no longer reachable).
    saved_depth: Option<usize>,
    max: usize,
}

impl<E> History<E> {
    pub fn new(max: usize) -> Self {
        History { undo: Vec::new(), redo: Vec::new(), saved_depth: Some(0), max: max.max(1) }
    }

    /// Applies `edits` to `model` and records them as one undo step. A step with the same `merge_key` as the previous one, pushed within
    /// [`MERGE_WINDOW`], joins it (typing or dragging a number is one undo step).
    pub fn push<M>(&mut self, model: &mut M, label: impl Into<String>, edits: Vec<E>, merge_key: Option<String>)
    where
        E: Edit<M>,
    {
        self.push_at(model, label.into(), edits, merge_key, Instant::now());
    }

    pub(crate) fn push_at<M>(&mut self, model: &mut M, label: String, edits: Vec<E>, merge_key: Option<String>, now: Instant)
    where
        E: Edit<M>,
    {
        if edits.is_empty() {
            return;
        }
        for e in &edits {
            e.apply(model);
        }
        self.redo.clear();
        if let Some(key) = &merge_key {
            if let Some(last) = self.undo.last_mut() {
                if last.key.as_deref() == Some(key) && now.saturating_duration_since(last.at) < MERGE_WINDOW {
                    last.edits.extend(edits);
                    last.at = now;
                    // the step the saved state sat on has changed: that state is gone
                    if self.saved_depth == Some(self.undo.len()) {
                        self.saved_depth = None;
                    }
                    return;
                }
            }
        }
        // A step that sits above the saved state while the redo stack is dropped makes the saved state unreachable.
        if let Some(d) = self.saved_depth {
            if d > self.undo.len() {
                self.saved_depth = None;
            }
        }
        self.undo.push(Step { label, edits, key: merge_key, at: now });
        if self.undo.len() > self.max {
            self.undo.remove(0);
            self.saved_depth = match self.saved_depth {
                Some(0) | None => None,
                Some(d) => Some(d - 1),
            };
        }
    }

    /// Undoes the last step; returns its label.
    pub fn undo<M>(&mut self, model: &mut M) -> Option<String>
    where
        E: Edit<M>,
    {
        let step = self.undo.pop()?;
        for e in step.edits.iter().rev() {
            e.invert().apply(model);
        }
        let label = step.label.clone();
        self.redo.push(step);
        Some(label)
    }

    pub fn redo<M>(&mut self, model: &mut M) -> Option<String>
    where
        E: Edit<M>,
    {
        let mut step = self.redo.pop()?;
        for e in &step.edits {
            e.apply(model);
        }
        step.key = None;
        let label = step.label.clone();
        self.undo.push(step);
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The document was saved at this point.
    pub fn mark_saved(&mut self) {
        self.saved_depth = Some(self.undo.len());
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_depth != Some(self.undo.len())
    }

    /// Labels of the undo steps, oldest first.
    pub fn labels(&self) -> Vec<String> {
        self.undo.iter().map(|s| s.label.clone()).collect()
    }

    /// Number of undo steps.
    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model is a number; an edit adds a delta.
    #[derive(Clone, Debug, PartialEq)]
    struct Add(i32);

    impl Edit<i32> for Add {
        fn apply(&self, m: &mut i32) {
            *m += self.0;
        }
        fn invert(&self) -> Self {
            Add(-self.0)
        }
    }

    #[test]
    fn undo_redo_and_dirty_flag() {
        let (mut m, mut h) = (0, History::<Add>::new(100));
        assert!(!h.is_dirty() && !h.can_undo());
        h.push(&mut m, "plus 5", vec![Add(5)], None);
        h.push(&mut m, "plus 2 and 3", vec![Add(2), Add(3)], None);
        assert_eq!((m, h.is_dirty(), h.labels()), (10, true, vec!["plus 5".to_string(), "plus 2 and 3".to_string()]));
        h.mark_saved();
        assert!(!h.is_dirty());
        assert_eq!(h.undo(&mut m).as_deref(), Some("plus 2 and 3"));
        assert_eq!((m, h.is_dirty(), h.can_redo()), (5, true, true));
        assert_eq!(h.redo(&mut m).as_deref(), Some("plus 2 and 3"));
        assert_eq!((m, h.is_dirty()), (10, false), "back at the saved depth");
        h.undo(&mut m);
        h.undo(&mut m);
        assert_eq!(m, 0);
        assert!(h.undo(&mut m).is_none());
        assert!(h.redo(&mut m).is_some());
    }

    #[test]
    fn a_new_edit_after_undo_makes_the_saved_state_unreachable() {
        let (mut m, mut h) = (0, History::<Add>::new(100));
        h.push(&mut m, "a", vec![Add(1)], None);
        h.push(&mut m, "b", vec![Add(1)], None);
        h.mark_saved();
        h.undo(&mut m);
        h.push(&mut m, "c", vec![Add(10)], None);
        assert!(h.is_dirty());
        h.undo(&mut m);
        h.redo(&mut m);
        assert!(h.is_dirty(), "the saved document is gone even at the same depth");
        assert!(!h.can_redo() || m == 11);
    }

    #[test]
    fn steps_with_the_same_key_inside_the_window_merge() {
        let (mut m, mut h) = (0, History::<Add>::new(100));
        let t0 = Instant::now();
        h.push_at(&mut m, "typing".into(), vec![Add(1)], Some("field".into()), t0);
        h.push_at(&mut m, "typing".into(), vec![Add(1)], Some("field".into()), t0 + Duration::from_millis(500));
        h.push_at(&mut m, "typing".into(), vec![Add(1)], Some("field".into()), t0 + Duration::from_millis(1000));
        assert_eq!((m, h.len()), (3, 1), "one undo step");
        // past the window, or another key: a new step
        h.push_at(&mut m, "typing".into(), vec![Add(1)], Some("field".into()), t0 + Duration::from_millis(1000) + MERGE_WINDOW);
        h.push_at(&mut m, "other".into(), vec![Add(1)], Some("other".into()), t0 + Duration::from_millis(1100) + MERGE_WINDOW);
        assert_eq!(h.len(), 3);
        h.undo(&mut m);
        h.undo(&mut m);
        assert_eq!(m, 3, "the merged step undoes as one");
        h.undo(&mut m);
        assert_eq!(m, 0);
        // merging into the step the save sat on makes the document dirty
        let (mut m, mut h) = (0, History::<Add>::new(100));
        h.push_at(&mut m, "x".into(), vec![Add(1)], Some("k".into()), t0);
        h.mark_saved();
        h.push_at(&mut m, "x".into(), vec![Add(1)], Some("k".into()), t0 + Duration::from_millis(100));
        assert!(h.is_dirty());
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn the_history_is_capped() {
        let (mut m, mut h) = (0, History::<Add>::new(3));
        for i in 0..5 {
            h.push(&mut m, format!("s{i}"), vec![Add(1)], None);
        }
        assert_eq!(h.labels(), vec!["s2", "s3", "s4"]);
        assert!(h.is_dirty());
        // a saved state that fell off the end is unreachable; one inside the window still is
        h.mark_saved();
        h.push(&mut m, "s5".to_string(), vec![Add(1)], None);
        assert!(h.is_dirty());
        h.undo(&mut m);
        assert!(!h.is_dirty());
    }
}
