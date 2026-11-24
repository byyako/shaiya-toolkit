//! One open map: the model, its undo history, the selection, the validation findings and the file it came from. No GUI here,
//! so the edit/undo/save behaviour is testable on its own.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use shaiya_svmap::{Finding, SectionId, Svmap};

use crate::edit::{self, Op, OpKind, Rec};
use crate::pair::{self, COp, ClientPart};

/// A selected record: section and index.
pub type Sel = (SectionId, usize);

const MERGE_WINDOW_MS: u128 = 1500;
const MAX_UNDO: usize = 5000;

pub struct Step {
    pub label: String,
    pub ops: Vec<Op>,
    /// The client side of a paired step: applied and undone together with `ops`.
    pub cops: Vec<COp>,
    key: Option<String>,
    /// When the step was made (the Changes list shows "n s ago").
    pub at: Instant,
}

/// File identity when it was read or written: modification time and length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let md = std::fs::metadata(path).ok()?;
    Some(Stamp { modified: md.modified().ok(), len: md.len() })
}

/// The 2D camera of a tab: world (x, z) at the centre of the viewport and pixels per world unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub cx: f32,
    pub cz: f32,
    pub zoom: f32,
    /// False until the map has been fitted into the viewport once.
    pub fitted: bool,
}

impl Default for View {
    fn default() -> Self {
        View { cx: 0.0, cz: 0.0, zoom: 1.0, fitted: false }
    }
}

/// The client half of a tab.
pub enum ClientSlot {
    /// No client folder, no `.wld` for this map, or not looked at yet.
    None,
    /// The `.wld` is there and cannot be written back byte for byte (the reason): client edits are refused.
    ReadOnly(String),
    Part(Box<ClientPart>),
}

pub struct Doc {
    /// Unique per open tab.
    pub uid: u64,
    pub map_id: Option<u32>,
    pub path: Option<PathBuf>,
    pub map: Svmap,
    /// The map as loaded or last saved (the Changes list diffs against it).
    pub base: Svmap,
    pub stamp: Option<Stamp>,
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    saved_depth: Option<usize>,
    pub selection: Vec<Sel>,
    /// Selected point / box inside the single selected record (NPC path point, bind box).
    pub sub: Option<usize>,
    pub findings: Vec<Finding>,
    pub validate_due: Option<Instant>,
    pub bak_done: bool,
    pub view: View,
    /// Rows of the blocked grid changed since the viewport texture was last updated: (x0, z0, x1, z1), half-open.
    pub blocked_dirty: Option<(u32, u32, u32, u32)>,
    /// Counts every change of the blocked grid (the 3D view re-uploads its cell texture when this moves).
    pub blocked_rev: u64,
    /// Bumped by every commit, undo, redo, revert and reload: autosave and the compare cache look at it.
    pub edit_gen: u64,
    /// Bumped by every save and reload (the "last save" the Changes list compares against changed).
    pub base_gen: u64,
    /// When the last edit happened.
    pub edit_at: Instant,
    /// How long after a step an edit of the same key still merges into it (milliseconds). Tests raise it so a slow machine cannot split a merge.
    pub merge_window_ms: u128,
    /// A tab to look and copy from: every edit is refused (`refused` counts the attempts, for the app's toast).
    pub read_only: bool,
    pub refused: u32,
    /// The disk stamp the "changed on disk" bar has acknowledged (Keep mine moves it without touching `stamp`, so a save still asks).
    pub seen: Option<Stamp>,
    /// A name for tabs that are not a plain map file (a backup or reference copy).
    pub label: Option<String>,
    /// When the last save was read back and found equal to the map (the status bar says "Saved and verified").
    pub verified_at: Option<Instant>,
    /// The client `.wld` and `<id>.txt` of this map, edited together with the svmap.
    pub client: ClientSlot,
    /// The undo depth the client part was last saved at (`None`: that state cannot be reached any more).
    client_saved: Option<usize>,
    /// Why the last client save failed (the status bar shows it until the next good save).
    pub client_error: Option<String>,
    /// The client folder this tab's client half was read for (so it is read again when another client is opened).
    pub client_for: Option<PathBuf>,
    /// The selected placements of the client half (the Placements tool). Indices shift when a placement is added or removed, so a step
    /// that does that, and undo or redo of it, clears this list; the tool selects what it made itself.
    pub place_sel: Vec<crate::place::PSel>,
}

impl Doc {
    pub fn new(uid: u64, map_id: Option<u32>, path: Option<PathBuf>, map: Svmap, stamp: Option<Stamp>) -> Doc {
        Doc {
            uid,
            map_id,
            path,
            base: map.clone(),
            map,
            stamp,
            undo: Vec::new(),
            redo: Vec::new(),
            saved_depth: Some(0),
            selection: Vec::new(),
            sub: None,
            findings: Vec::new(),
            validate_due: Some(Instant::now()),
            bak_done: false,
            view: View::default(),
            blocked_dirty: None,
            blocked_rev: 0,
            edit_gen: 0,
            base_gen: 0,
            edit_at: Instant::now(),
            merge_window_ms: MERGE_WINDOW_MS,
            read_only: false,
            refused: 0,
            seen: stamp,
            label: None,
            verified_at: None,
            client: ClientSlot::None,
            client_saved: Some(0),
            client_error: None,
            client_for: None,
            place_sel: Vec::new(),
        }
    }

    /// Reads a `.svmap`; the map id is the file's number.
    pub fn open(uid: u64, path: &Path) -> Result<Doc, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let map = shaiya_svmap::read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let id = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
        Ok(Doc::new(uid, id, Some(path.to_path_buf()), map, stamp_of(path)))
    }

    pub fn title(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        match (&self.path, self.map_id) {
            (Some(p), _) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            (None, Some(id)) => format!("{id}.svmap (new)"),
            (None, None) => "new map".to_string(),
        }
    }

    /// A map that has no file yet: it counts as modified until it is saved. `base` (what the Changes list diffs against) stays
    /// as given, so every record shows up as added.
    pub fn new_unsaved(uid: u64, map_id: Option<u32>, map: Svmap) -> Doc {
        let mut d = Doc::new(uid, map_id, None, map, None);
        d.saved_depth = None;
        d
    }

    /// Unsaved changes in the svmap or in the client part.
    pub fn is_dirty(&self) -> bool {
        self.svmap_dirty() || self.client_dirty()
    }

    /// The svmap has unsaved changes.
    pub fn svmap_dirty(&self) -> bool {
        let Some(d) = self.saved_depth else { return true };
        let n = self.undo.len();
        // only steps that changed the svmap count: a rename in the client half leaves the svmap as it was saved
        if n >= d {
            self.undo[d..].iter().any(|s| !s.ops.is_empty())
        } else {
            self.redo.iter().rev().take(d - n).any(|s| !s.ops.is_empty())
        }
    }

    /// The client part has unsaved changes. Counted from the steps since the last save, so it costs nothing per frame.
    pub fn client_dirty(&self) -> bool {
        if !matches!(self.client, ClientSlot::Part(_)) {
            return false;
        }
        let Some(d) = self.client_saved else { return true };
        let n = self.undo.len();
        if n >= d {
            self.undo[d..].iter().any(|s| !s.cops.is_empty())
        } else {
            self.redo.iter().rev().take(d - n).any(|s| !s.cops.is_empty())
        }
    }

    pub fn part(&self) -> Option<&ClientPart> {
        match &self.client {
            ClientSlot::Part(p) => Some(p),
            _ => None,
        }
    }

    pub fn part_mut(&mut self) -> Option<&mut ClientPart> {
        match &mut self.client {
            ClientSlot::Part(p) => Some(p),
            _ => None,
        }
    }

    /// Gives the tab its client half; what it holds now counts as saved.
    pub fn set_client(&mut self, slot: ClientSlot) {
        self.client = slot;
        // The client ops in the history were made against the half this tab held before (another map number after Save as,
        // another client folder, or the same file read again). Applied to the new half by undo or redo they would write records
        // of the old one by index, or insert a record twice: the history keeps its svmap side only.
        for s in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            s.cops.clear();
        }
        self.client_saved = Some(self.undo.len());
        self.client_error = None;
        self.place_sel.clear();
    }

    /// The client part was written: its state at this depth is the saved one.
    pub fn mark_client_saved(&mut self) {
        self.client_saved = Some(self.undo.len());
        self.client_error = None;
        if let Some(l) = self.undo.last_mut() {
            l.key = None;
        }
        if let Some(p) = self.part_mut() {
            p.rebase();
        }
    }

    /// Whether a client edit can be made on this tab, or why not (a read-only tab, a `.wld` that does not write back byte for
    /// byte, or no client map).
    pub fn client_edit_check(&self) -> Result<(), String> {
        if self.read_only {
            return Err("this tab is read-only".to_string());
        }
        match &self.client {
            ClientSlot::Part(_) => Ok(()),
            ClientSlot::ReadOnly(why) => Err(format!("the client map is read-only: {why}")),
            ClientSlot::None => Err("no client map is open for this map".to_string()),
        }
    }

    /// Remembers that the cells in this half-open rectangle changed (the app updates the overlay texture from it).
    pub fn mark_blocked_dirty(&mut self, r: (u32, u32, u32, u32)) {
        self.blocked_rev += 1;
        self.blocked_dirty = Some(match self.blocked_dirty {
            None => r,
            Some(o) => (o.0.min(r.0), o.1.min(r.1), o.2.max(r.2), o.3.max(r.3)),
        });
    }

    /// Makes `after` the blocked grid as one undo step (the cells that differ are the step). Returns the number of cells
    /// changed; nothing happens when the grid is already `after` or is of another size.
    pub fn commit_blocked(&mut self, label: &str, after: &shaiya_svmap::BitGrid) -> u64 {
        match self.map.blocked.diff_runs(after) {
            Some(runs) if !runs.is_empty() => {
                let n = runs.iter().map(|r| r.len).sum();
                self.commit(label, vec![Op::cells(runs)]);
                n
            }
            _ => 0,
        }
    }

    fn mark_dirty_for(&mut self, ops: &[Op]) {
        for o in ops {
            if let OpKind::Cells(runs) = &o.kind {
                if let Some(b) = self.map.blocked.runs_bounds(runs) {
                    self.mark_blocked_dirty(b);
                }
            }
        }
    }

    // ------------------------------------------------------------------ edits

    /// Applies `ops` and records them as one undo step. With a client part the same change is made on the paired sections of the
    /// `.wld` in the same step (`pair::mirror`).
    pub fn commit(&mut self, label: impl Into<String>, ops: Vec<Op>) {
        self.push(label.into(), ops, None, true, Vec::new());
    }

    /// Like [`commit`](Self::commit) but the client part stays as it is: the edit is on the svmap side only (Alt-drag, Sync
    /// "take client").
    pub fn commit_server_only(&mut self, label: impl Into<String>, ops: Vec<Op>) {
        self.push(label.into(), ops, None, false, Vec::new());
    }

    /// An undo step that changes the client part only (a destination name, an area name, Sync "take server"). Refused with the
    /// reason when the tab has no editable client map.
    pub fn commit_client(&mut self, label: impl Into<String>, cops: Vec<COp>) -> Result<(), String> {
        if self.read_only {
            self.refused += 1;
        }
        self.client_edit_check()?;
        self.push(label.into(), Vec::new(), None, false, cops);
        Ok(())
    }

    /// Like [`commit_client`](Self::commit_client), but a replace of the same records with the same `key` within a short time merges
    /// into the previous step (typing or dragging a number in the Placements inspector is one undo step).
    pub fn commit_client_coalesced(&mut self, key: &str, label: impl Into<String>, cops: Vec<COp>) -> Result<(), String> {
        if cops.is_empty() {
            return Ok(());
        }
        if self.read_only {
            self.refused += 1;
        }
        self.client_edit_check()?;
        if let Some(last) = self.undo.last() {
            if last.ops.is_empty() && last.key.as_deref() == Some(key) && last.at.elapsed().as_millis() < self.merge_window_ms && pair::mergeable(&last.cops, &cops) {
                self.apply_cops(&cops);
                let last = self.undo.last_mut().expect("checked");
                let back = pair::merge_into(&mut last.cops, &cops);
                last.at = Instant::now();
                self.redo.clear();
                if back {
                    self.undo.pop();
                }
                self.touch();
                return Ok(());
            }
        }
        self.push(label.into(), Vec::new(), Some(key.to_string()), false, cops);
        Ok(())
    }

    /// A step with svmap ops that are not mirrored and client ops that were made by hand (Sync with both directions).
    pub fn commit_both(&mut self, label: impl Into<String>, ops: Vec<Op>, cops: Vec<COp>) -> Result<(), String> {
        if !cops.is_empty() {
            self.client_edit_check()?;
        }
        self.push(label.into(), ops, None, false, cops);
        Ok(())
    }

    /// The client ops a step of `ops` would make: the mirrored ones, then `extra`.
    fn derive_cops(&self, ops: &[Op], mirror: bool, extra: Vec<COp>) -> Vec<COp> {
        let Some(p) = self.part() else { return Vec::new() };
        // a step that touches no paired section needs no copy of the client lists (a paint stroke, a scatter)
        let paired_ops = ops.iter().any(|o| pair::slot(o.section()).is_some());
        let mut cops = if mirror && paired_ops {
            let paired = p.paired(&self.map);
            let mut l = p.lists_core();
            pair::mirror(&mut l, paired, ops)
        } else {
            Vec::new()
        };
        cops.extend(extra);
        cops
    }

    fn apply_cops(&mut self, cops: &[COp]) {
        if let Some(p) = self.part_mut() {
            for c in cops {
                p.apply(c);
            }
        }
    }

    /// Like [`commit`](Self::commit), but a single replace of the same record with the same `key` within a short time merges
    /// into the previous step (typing or dragging a number is one undo step).
    pub fn commit_coalesced(&mut self, key: &str, label: impl Into<String>, ops: Vec<Op>) {
        if ops.is_empty() {
            return;
        }
        if self.read_only {
            self.refused += 1;
            return;
        }
        // the same replaces (one record, or the same set of records for a batch edit) with the same key within a short time merge
        let cops = self.derive_cops(&ops, true, Vec::new());
        if let Some(last) = self.undo.last() {
            let same = last.ops.len() == ops.len() && last.ops.iter().zip(&ops).all(|(a, b)| a.index == b.index && matches!((&a.kind, &b.kind), (OpKind::Replace { .. }, OpKind::Replace { .. })));
            if same && last.key.as_deref() == Some(key) && last.at.elapsed().as_millis() < self.merge_window_ms && pair::mergeable(&last.cops, &cops) {
                self.apply_cops(&cops);
                let last = self.undo.last_mut().expect("checked");
                let mut back_to_start = true;
                for (old, new) in last.ops.iter_mut().zip(&ops) {
                    if let (OpKind::Replace { before, after }, OpKind::Replace { after: new_after, .. }) = (&mut old.kind, &new.kind) {
                        new.apply(&mut self.map);
                        *after = new_after.clone();
                        back_to_start &= before == after;
                    }
                }
                let client_back = pair::merge_into(&mut last.cops, &cops);
                last.at = Instant::now();
                self.redo.clear();
                if back_to_start && client_back {
                    self.undo.pop();
                }
                self.touch();
                return;
            }
        }
        self.push(label.into(), ops, Some(key.to_string()), true, Vec::new());
    }

    fn push(&mut self, label: String, ops: Vec<Op>, key: Option<String>, mirror: bool, extra: Vec<COp>) {
        if ops.is_empty() && extra.is_empty() {
            return;
        }
        if self.read_only {
            self.refused += 1;
            return;
        }
        let cops = self.derive_cops(&ops, mirror, extra);
        for o in &ops {
            o.apply(&mut self.map);
        }
        self.apply_cops(&cops);
        if crate::place::changes_structure(&cops) {
            self.place_sel.clear();
        }
        self.mark_dirty_for(&ops);
        // A step that sits above the saved state while the redo stack is dropped makes the saved state unreachable.
        if let Some(d) = self.saved_depth {
            let n = self.undo.len();
            if d > n {
                let dropped = self.redo.iter().rev().take(d - n).any(|s| !s.ops.is_empty());
                self.saved_depth = if dropped { None } else { Some(n) };
            }
        }
        // The same for the client part, unless the steps that go had nothing to do with it.
        if let Some(d) = self.client_saved {
            let n = self.undo.len();
            if d > n {
                let dropped_client = self.redo.iter().rev().take(d - n).any(|s| !s.cops.is_empty());
                self.client_saved = if dropped_client { None } else { Some(n) };
            }
        }
        self.redo.clear();
        self.undo.push(Step { label, ops, cops, key, at: Instant::now() });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
            self.saved_depth = match self.saved_depth {
                Some(0) | None => None,
                Some(d) => Some(d - 1),
            };
            self.client_saved = match self.client_saved {
                Some(0) | None => None,
                Some(d) => Some(d - 1),
            };
        }
        self.touch();
    }

    fn touch(&mut self) {
        self.edit_gen += 1;
        self.edit_at = Instant::now();
        self.validate_due = Some(Instant::now() + std::time::Duration::from_millis(200));
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undoes the last step; returns its label.
    pub fn undo(&mut self) -> Option<String> {
        let step = self.undo.pop()?;
        for o in step.ops.iter().rev() {
            o.invert().apply(&mut self.map);
        }
        if let Some(p) = self.part_mut() {
            for c in step.cops.iter().rev() {
                p.apply(&c.invert());
            }
        }
        if crate::place::changes_structure(&step.cops) {
            self.place_sel.clear();
        }
        self.mark_dirty_for(&step.ops);
        let label = step.label.clone();
        self.select_after(&step.ops, true);
        self.redo.push(step);
        self.touch();
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let mut step = self.redo.pop()?;
        for o in &step.ops {
            o.apply(&mut self.map);
        }
        self.apply_cops(&step.cops);
        if crate::place::changes_structure(&step.cops) {
            self.place_sel.clear();
        }
        self.mark_dirty_for(&step.ops);
        step.key = None;
        let label = step.label.clone();
        self.select_after(&step.ops, false);
        self.undo.push(step);
        self.touch();
        Some(label)
    }

    /// Undoes or redoes until `depth` steps are applied (0 = the file as opened or saved).
    pub fn jump_to(&mut self, depth: usize) {
        while self.undo.len() > depth {
            if self.undo().is_none() {
                break;
            }
        }
        while self.undo.len() < depth {
            if self.redo().is_none() {
                break;
            }
        }
    }

    /// Makes the map equal to `target` as one undo step built from the difference (per section: removes, inserts, replaces; the
    /// blocked cells as one cell-diff op). False when nothing differs or the size does (the size cannot be edited).
    pub fn replace_with(&mut self, label: &str, target: &Svmap) -> bool {
        match edit::diff_ops(&self.map, target) {
            Some(ops) if !ops.is_empty() => {
                self.commit(label, ops);
                true
            }
            _ => false,
        }
    }

    /// After undo/redo: select what the step touched and still exists.
    fn select_after(&mut self, ops: &[Op], undone: bool) {
        if ops.iter().all(|o| matches!(o.kind, OpKind::Cells(_))) {
            return; // a paint stroke does not change what is selected
        }
        let mut sel = Vec::new();
        for o in ops {
            let exists = matches!((&o.kind, undone), (OpKind::Replace { .. }, _) | (OpKind::Insert(_), false) | (OpKind::Remove(_), true));
            if exists {
                sel.push((o.section(), o.index));
            }
        }
        sel.retain(|(s, i)| *i < edit::count(&self.map, *s));
        sel.sort();
        sel.dedup();
        self.selection = sel;
        self.sub = None;
    }

    // ------------------------------------------------------------------ selection

    pub fn is_selected(&self, s: SectionId, i: usize) -> bool {
        self.selection.contains(&(s, i))
    }

    pub fn select_only(&mut self, s: SectionId, i: usize) {
        self.selection = vec![(s, i)];
        self.sub = None;
    }

    pub fn toggle(&mut self, s: SectionId, i: usize) {
        if let Some(p) = self.selection.iter().position(|x| *x == (s, i)) {
            self.selection.remove(p);
        } else {
            self.selection.push((s, i));
        }
        self.sub = None;
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.sub = None;
    }

    /// The only selected record, if exactly one is selected.
    pub fn single(&self) -> Option<Sel> {
        if self.selection.len() == 1 {
            Some(self.selection[0])
        } else {
            None
        }
    }

    /// Removes the selected records as one undo step.
    pub fn delete_selected(&mut self) -> usize {
        let mut sel = self.selection.clone();
        sel.sort();
        sel.dedup();
        let mut ops = Vec::new();
        for (s, i) in sel.iter().rev() {
            if let Some(r) = edit::get(&self.map, *s, *i) {
                ops.push(Op::remove(*i, r));
            }
        }
        let n = ops.len();
        if n > 0 {
            let label = if n == 1 { format!("delete {}", sel[0].0.label()) } else { format!("delete {n} records") };
            self.commit(label, ops);
        }
        self.clear_selection();
        n
    }

    // ------------------------------------------------------------------ saving

    /// Whether the file on disk differs from what was read or written last (time or size).
    pub fn changed_on_disk(&self) -> bool {
        match (&self.path, self.stamp) {
            (Some(p), Some(s)) => match stamp_of(p) {
                Some(now) => now != s,
                None => true,
            },
            _ => false,
        }
    }

    /// Writes to `path` (the document's own path when `None`). Returns the path written.
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        let target = match (path, &self.path) {
            (Some(p), _) => p.to_path_buf(),
            (None, Some(p)) => p.clone(),
            (None, None) => return Err("no file name yet: use Save As".to_string()),
        };
        let backup = !(self.bak_done && Some(&target) == self.path.as_ref());
        save_map(&self.map, &target, backup)?;
        self.bak_done = true;
        if self.path.as_deref() != Some(target.as_path()) {
            self.path = Some(target.clone());
            if let Some(id) = target.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
                self.map_id = Some(id);
            }
        }
        self.stamp = stamp_of(&target);
        self.seen = self.stamp;
        self.verified_at = None;
        // V1: read the file again and compare it with the map. On a mismatch the tab stays modified and the `.bak` stays.
        #[cfg(test)]
        WRITE_HOOK.with(|h| {
            if let Some(f) = h.borrow().as_ref() {
                f(&target);
            }
        });
        if let Err(why) = verify_file(&self.map, &target) {
            let mut bak = target.file_name().unwrap_or_default().to_os_string();
            bak.push(".bak");
            let bak = target.with_file_name(bak);
            let kept = if bak.is_file() { format!(" The backup {} stays.", bak.display()) } else { " There was no file there before, so there is no backup.".to_string() };
            return Err(format!("{} was written but does not read back as the map. The tab stays modified.{kept} {why}", target.display()));
        }
        self.verified_at = Some(Instant::now());
        self.base_gen += 1;
        self.base = self.map.clone();
        self.saved_depth = Some(self.undo.len());
        if let Some(l) = self.undo.last_mut() {
            l.key = None;
        }
        Ok(target)
    }

    /// Reads the file again and drops every unsaved change and the undo history ("changed on disk > Reload").
    pub fn reload_from_disk(&mut self) -> Result<(), String> {
        let path = self.path.clone().ok_or("this map has no file")?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let map = shaiya_svmap::read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = map.size;
        self.base = map.clone();
        self.map = map;
        // the unsaved client edits go with the history: the client part returns to what it was when it was read or saved
        if let Some(p) = self.part_mut() {
            let (cur, base) = (p.lists(), p.base.clone());
            if cur != base {
                p.apply(&COp::Lists { before: Box::new(cur), after: Box::new(base) });
            }
        }
        self.client_saved = Some(0);
        self.undo.clear();
        self.redo.clear();
        self.saved_depth = Some(0);
        self.selection.clear();
        self.place_sel.clear();
        self.sub = None;
        self.stamp = stamp_of(&path);
        self.seen = self.stamp;
        self.base_gen += 1;
        self.mark_blocked_dirty((0, 0, size, size));
        self.touch();
        Ok(())
    }

    /// A tab for work recovered from an earlier session: the recovered map is the document, marked as modified. With the file as
    /// it is on disk (`disk`) one undo step "recovered edits" leads back to it; without (the file is gone, or a new map) the
    /// Changes list shows everything as added.
    pub fn from_recovery(uid: u64, map_id: Option<u32>, path: Option<PathBuf>, recovered: Svmap, disk: Option<Svmap>) -> Doc {
        let stamp = path.as_deref().and_then(stamp_of);
        match disk {
            Some(disk) => {
                let mut d = Doc::new(uid, map_id, path, disk, stamp);
                if !d.replace_with("recovered edits", &recovered) && d.map.size != recovered.size {
                    // another size than the file: no step can lead back, so the recovered map is adopted as it is
                    d.mark_blocked_dirty((0, 0, recovered.size, recovered.size));
                    d.map = recovered;
                    d.saved_depth = None;
                    d.edit_gen += 1;
                }
                d
            }
            None => {
                let empty = Svmap::new(recovered.size);
                let mut d = Doc::new(uid, map_id, path, recovered, stamp);
                d.base = empty;
                d.saved_depth = None;
                d.edit_gen = 1;
                d
            }
        }
    }

    /// Records changed since the last save/load, as the library's diff.
    #[cfg(test)]
    pub fn changes(&self) -> Vec<shaiya_svmap::diff::DiffEntry> {
        shaiya_svmap::diff::diff(&self.base, &self.map)
    }
}

#[cfg(test)]
type WriteHook = Option<Box<dyn Fn(&Path)>>;
#[cfg(test)]
thread_local! {
    /// Test hook: runs after a save wrote the file and before it is read back (the test flips a byte).
    static WRITE_HOOK: std::cell::RefCell<WriteHook> = const { std::cell::RefCell::new(None) };
}

/// V1: reads the saved file again and compares it with the map in memory. The error names the records that differ.
pub fn verify_file(map: &Svmap, path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("The file cannot be read again: {e}."))?;
    // byte-equal to what the map writes: the strictest check (it also catches bytes the parser would skip)
    if bytes == shaiya_svmap::write(map) {
        return Ok(());
    }
    let back = shaiya_svmap::read(&bytes).map_err(|e| format!("The file cannot be parsed again: {e}."))?;
    let mut parts: Vec<String> = Vec::new();
    if back.size != map.size {
        parts.push(format!("size {} instead of {}", back.size, map.size));
    }
    for r in crate::compare::rows(map, &back).iter().take(6) {
        parts.push(format!("{} #{}", r.section.label(), r.cur_index));
    }
    if back.blocked != map.blocked {
        parts.push("blocked cells".to_string());
    }
    if parts.is_empty() {
        parts.push("some other bytes".to_string());
    }
    Err(format!("Differs: {}.", parts.join(", ")))
}

/// Safe save: `<file>.tmp` first, re-read and compared, the original copied to `<file>.bak` when there is none, then the
/// temporary file replaces the original.
pub fn save_map(map: &Svmap, path: &Path, backup: bool) -> Result<(), String> {
    let bytes = shaiya_svmap::write(map);
    let mut tmp_name = path.file_name().ok_or("no file name")?.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, &bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    let back = std::fs::read(&tmp).map_err(|e| format!("cannot re-read {}: {e}", tmp.display()))?;
    if back != bytes || shaiya_svmap::read(&back).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{} did not read back identically; the original was not touched", tmp.display()));
    }
    if backup && path.is_file() {
        let mut bak_name = path.file_name().ok_or("no file name")?.to_os_string();
        bak_name.push(".bak");
        let bak = path.with_file_name(bak_name);
        if !bak.exists() {
            std::fs::copy(path, &bak).map_err(|e| format!("cannot write the backup {}: {e}", bak.display()))?;
        }
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        // a read-only or locked target: the original stays as it was and no stray .tmp is left behind
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot replace {}: {e}", path.display()));
    }
    Ok(())
}

/// Ops that move records by a delta: one replace per record, taken from the given snapshots.
pub fn move_ops(snapshots: &[(Sel, Rec)], map: &Svmap) -> Vec<Op> {
    let mut ops = Vec::new();
    for ((s, i), before) in snapshots {
        if let Some(after) = edit::get(map, *s, *i) {
            if &after != before {
                ops.push(Op::replace(*i, before.clone(), after));
            }
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::{MobArea, MobEntry, NpcEntry, NpcPoint, Portal};

    fn sample() -> Svmap {
        let mut m = Svmap::new(128);
        m.mob_areas.push(MobArea { min: [10.0, 0.0, 10.0], max: [20.0, 2.0, 20.0], mobs: vec![MobEntry { mob: 5, count: 3 }] });
        m.npcs.push(NpcEntry { kind: 1, type_id: 2, points: vec![NpcPoint { pos: [30.0, 1.0, 30.0], dir: 1.0 }] });
        m.portals.push(Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 1, level_max: 5, to_map: 2, to_pos: [9.0, 0.0, 9.0] });
        m
    }

    fn doc() -> Doc {
        Doc::new(1, Some(9), None, sample(), None)
    }

    #[test]
    fn undo_redo_dirty() {
        let mut d = doc();
        assert!(!d.is_dirty());
        let before = edit::get(&d.map, SectionId::MobAreas, 0).unwrap();
        let mut after = before.clone();
        edit::translate(&mut after, 4.0, 4.0);
        d.commit("move", vec![Op::replace(0, before, after)]);
        assert!(d.is_dirty());
        assert_eq!(d.map.mob_areas[0].min[0], 14.0);
        d.undo();
        assert!(!d.is_dirty());
        assert_eq!(d.map, d.base);
        d.redo();
        assert!(d.is_dirty());
        assert_eq!(d.map.mob_areas[0].min[0], 14.0);
    }

    #[test]
    fn coalesce_and_back_to_start() {
        let mut d = doc();
        d.merge_window_ms = u128::MAX; // no real clock in the test
        for v in [11.0, 12.0, 13.0] {
            let before = edit::get(&d.map, SectionId::MobAreas, 0).unwrap();
            let mut after = before.clone();
            if let Rec::Mob(a) = &mut after {
                a.min[0] = v;
            }
            d.commit_coalesced("mob0x1", "area x1", vec![Op::replace(0, before, after)]);
        }
        assert_eq!(d.undo.len(), 1);
        // typing the old value again removes the step
        let before = edit::get(&d.map, SectionId::MobAreas, 0).unwrap();
        let mut after = before.clone();
        if let Rec::Mob(a) = &mut after {
            a.min[0] = 10.0;
        }
        d.commit_coalesced("mob0x1", "area x1", vec![Op::replace(0, before, after)]);
        assert_eq!(d.undo.len(), 0);
        assert!(!d.is_dirty());
    }

    #[test]
    fn delete_several_and_undo() {
        let mut d = doc();
        d.selection = vec![(SectionId::MobAreas, 0), (SectionId::Npcs, 0), (SectionId::Portals, 0)];
        assert_eq!(d.delete_selected(), 3);
        assert!(d.map.mob_areas.is_empty() && d.map.npcs.is_empty() && d.map.portals.is_empty());
        d.undo();
        assert_eq!(d.map, d.base);
    }

    #[test]
    fn save_roundtrip_with_backup() {
        let guard = crate::testutil::TempDir::new("doc-save-roundtrip");
        let dir = guard.path().to_path_buf();
        let file = dir.join("9.svmap");
        std::fs::write(&file, shaiya_svmap::write(&sample())).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        assert_eq!(d.map_id, Some(9));
        // saving without edits keeps the bytes
        let orig = std::fs::read(&file).unwrap();
        d.save(None).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), orig);
        assert!(dir.join("9.svmap.bak").exists());
        // an edit changes the file, the backup keeps the original
        let before = edit::get(&d.map, SectionId::SectionB, 0);
        assert!(before.is_none());
        d.commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
        assert!(d.is_dirty());
        d.save(None).unwrap();
        assert!(!d.is_dirty());
        assert_ne!(std::fs::read(&file).unwrap(), orig);
        assert_eq!(std::fs::read(dir.join("9.svmap.bak")).unwrap(), orig);
        assert!(!dir.join("9.svmap.tmp").exists());
        assert!(!d.changed_on_disk());
    }

    /// Copy of an original map in a temp dir of the calling test's own (`tag` names the test; the directory goes away with the
    /// guard), or `None` when `SVMAP_ORIGINALS` is not set. The originals are only read.
    fn original_copy(tag: &str, id: u32) -> Option<(crate::testutil::TempDir, PathBuf)> {
        let dir = shaiya_testkit::env_var_path("SVMAP_ORIGINALS")?;
        let src = dir.join(format!("{id}.svmap"));
        let tmp = crate::testutil::TempDir::new(tag);
        let dst = tmp.join(&format!("{id}.svmap"));
        std::fs::copy(&src, &dst).ok()?;
        Some((tmp, dst))
    }

    #[test]
    fn map1_open_save_is_byte_identical() {
        let Some((guard, file)) = original_copy("map1-open-save", 1) else { return };
        let dir = guard.path().to_path_buf();
        let orig = std::fs::read(&file).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        assert_eq!(d.map_id, Some(1));
        d.save(None).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), orig, "open + save without edits must not change the file");
        assert_eq!(std::fs::read(dir.join("1.svmap.bak")).unwrap(), orig);
    }

    #[test]
    fn map1_scripted_edits_undo_to_the_original_bytes() {
        let Some((guard, file)) = original_copy("map1-scripted-edits", 1) else { return };
        let dir = guard.path().to_path_buf();
        let orig = std::fs::read(&file).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        assert!(!d.map.mob_areas.is_empty() && !d.map.portals.is_empty() && !d.map.npcs.is_empty());

        // 1: move a monster area
        let before = edit::get(&d.map, SectionId::MobAreas, 10).unwrap();
        let mut after = before.clone();
        edit::translate(&mut after, 12.5, -7.25);
        d.commit("move area", vec![Op::replace(10, before, after)]);
        // 2: change a portal destination
        let before = edit::get(&d.map, SectionId::Portals, 0).unwrap();
        let mut after = before.clone();
        if let Rec::Portal(p) = &mut after {
            p.to_map = 18;
            p.to_pos = [100.5, 2.0, 200.25];
        }
        d.commit("portal destination", vec![Op::replace(0, before, after)]);
        // 3: add a path point to an NPC
        let before = edit::get(&d.map, SectionId::Npcs, 3).unwrap();
        let mut after = before.clone();
        if let Rec::Npc(n) = &mut after {
            let last = *n.points.last().unwrap();
            n.points.push(shaiya_svmap::NpcPoint { pos: [last.pos[0] + 3.0, last.pos[1], last.pos[2]], dir: 1.0 });
        }
        d.commit("add path point", vec![Op::replace(3, before, after)]);
        // 4: add and delete records
        d.commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
        d.selection = vec![(SectionId::MobAreas, 5), (SectionId::Npcs, 7)];
        d.delete_selected();

        let edited = shaiya_svmap::write(&d.map);
        assert_ne!(edited, orig);
        assert!(d.is_dirty());
        assert_eq!(d.undo.len(), 5);
        while d.undo().is_some() {}
        assert_eq!(shaiya_svmap::write(&d.map), orig, "undoing every step restores the original bytes");
        assert!(!d.is_dirty());
        while d.redo().is_some() {}
        assert_eq!(shaiya_svmap::write(&d.map), edited, "redo reapplies every step");

        // saving the edited map and reading it back
        d.save(None).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), edited);
        assert_eq!(std::fs::read(dir.join("1.svmap.bak")).unwrap(), orig);
        let back = Doc::open(2, &file).unwrap();
        assert_eq!(back.map, d.map);
    }

    #[test]
    fn blocked_cell_steps_undo_redo_and_mark_dirty() {
        let mut d = doc();
        d.select_only(SectionId::Npcs, 0);
        let base = d.map.clone();
        let mut g = d.map.blocked.clone();
        g.fill_rect(10, 20, 30, 25, true);
        assert_eq!(d.commit_blocked("block rectangle", &g), 100);
        assert_eq!(d.map.blocked.count(), 100);
        assert_eq!(d.blocked_dirty, Some((10, 20, 30, 25)));
        assert!(d.is_dirty());
        assert_eq!(d.commit_blocked("again", &g), 0, "an unchanged grid is not a step");
        // a second step on top, then undo both
        let mut g2 = d.map.blocked.clone();
        g2.fill_all(false);
        d.commit_blocked("clear map", &g2);
        assert_eq!(d.map.blocked.count(), 0);
        d.blocked_dirty = None;
        d.undo();
        assert_eq!(d.map.blocked.count(), 100);
        assert_eq!(d.blocked_dirty, Some((10, 20, 30, 25)));
        d.undo();
        assert_eq!(d.map, base);
        assert!(!d.is_dirty());
        assert_eq!(d.selection, vec![(SectionId::Npcs, 0)], "a paint step leaves the selection alone");
        d.redo();
        d.redo();
        assert_eq!(d.map.blocked.count(), 0);
        // blocked edits are part of the diff against the saved file
        d.undo();
        assert!(d.changes().iter().any(|c| c.section == SectionId::Blocked));
    }

    /// V1: a save reads the file again and compares it with the map. A write that changed a byte is reported, the tab stays
    /// modified and the backup stays.
    #[test]
    fn save_verifies_after_write() {
        let guard = crate::testutil::TempDir::new("doc-verify");
        let file = guard.join("9.svmap");
        std::fs::write(&file, shaiya_svmap::write(&sample())).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        let before = edit::get(&d.map, SectionId::MobAreas, 0).unwrap();
        let mut after = before.clone();
        edit::translate(&mut after, 4.0, 0.0);
        d.commit("move", vec![Op::replace(0, before, after)]);
        d.save(None).unwrap();
        assert!(d.verified_at.is_some() && !d.is_dirty(), "a good save is verified");
        assert!(verify_file(&d.map, &file).is_ok());
        // a write that flips one byte (one that still parses)
        let cur = edit::get(&d.map, SectionId::MobAreas, 0).unwrap();
        let mut moved = cur.clone();
        edit::translate(&mut moved, 9.0, 0.0);
        d.commit("move again", vec![Op::replace(0, cur, moved)]);
        let orig_bak = std::fs::read(guard.join("9.svmap.bak")).unwrap();
        let want = shaiya_svmap::write(&d.map);
        WRITE_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |p: &Path| {
                let mut b = std::fs::read(p).unwrap();
                for k in (0..b.len()).rev() {
                    b[k] ^= 1;
                    if shaiya_svmap::read(&b).is_ok_and(|m| shaiya_svmap::write(&m) != want) {
                        break;
                    }
                    b[k] ^= 1;
                }
                std::fs::write(p, b).unwrap();
            }));
        });
        let e = d.save(None).unwrap_err();
        WRITE_HOOK.with(|h| *h.borrow_mut() = None);
        assert!(e.contains("does not read back") && e.contains("Differs"), "{e}");
        assert!(d.is_dirty() && d.verified_at.is_none(), "the tab stays modified");
        assert_eq!(d.undo.len(), 2);
        assert_eq!(std::fs::read(guard.join("9.svmap.bak")).unwrap(), orig_bak, "the backup stays");
        // without the hook the next save is fine again
        d.save(None).unwrap();
        assert!(!d.is_dirty() && d.verified_at.is_some());
    }

    /// A target with the read-only attribute: the save fails, the file keeps its bytes, no `.tmp` stays, the tab stays modified.
    #[test]
    fn save_onto_a_read_only_file_leaves_it_alone() {
        let guard = crate::testutil::TempDir::new("doc-ro-file");
        let file = guard.join("9.svmap");
        let orig = shaiya_svmap::write(&sample());
        std::fs::write(&file, &orig).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        d.commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
        let mut perm = std::fs::metadata(&file).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&file, perm.clone()).unwrap();
        let r = d.save(None);
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&file, perm).unwrap();
        if cfg!(windows) {
            assert!(r.is_err(), "{r:?}");
            assert_eq!(std::fs::read(&file).unwrap(), orig, "the original is untouched");
            assert!(!guard.join("9.svmap.tmp").exists(), "no stray .tmp");
            assert!(d.is_dirty() && d.verified_at.is_none());
        }
    }

    #[test]
    fn unsaved_new_map_is_dirty_until_saved() {
        let guard = crate::testutil::TempDir::new("doc-new-unsaved");
        let dir = guard.path().to_path_buf();
        let mut d = Doc::new_unsaved(3, Some(77), Svmap::new(64));
        assert!(d.is_dirty() && d.path.is_none());
        assert!(d.save(None).is_err(), "no file name yet");
        d.save(Some(&dir.join("77.svmap"))).unwrap();
        assert!(!d.is_dirty());
        assert_eq!(d.map_id, Some(77));
    }

    fn mover(d: &Doc, dx: f32) -> Vec<Op> {
        let before = edit::get(&d.map, SectionId::MobAreas, 0).unwrap();
        let mut after = before.clone();
        edit::translate(&mut after, dx, 0.0);
        vec![Op::replace(0, before, after)]
    }

    #[test]
    fn history_jump() {
        let mut d = doc();
        let start = d.map.clone();
        let mut states = vec![start];
        for k in 0..5 {
            let ops = mover(&d, 1.0 + k as f32);
            d.commit(format!("step {k}"), ops);
            states.push(d.map.clone());
        }
        assert_eq!(d.undo.len(), 5);
        let gen0 = d.edit_gen;
        d.jump_to(2);
        assert_eq!((d.undo.len(), d.redo.len()), (2, 3));
        assert_eq!(d.map, states[2]);
        assert!(d.edit_gen > gen0, "a jump counts as an edit");
        d.jump_to(5);
        assert_eq!((d.undo.len(), d.redo.len()), (5, 0));
        assert_eq!(d.map, states[5]);
        d.jump_to(0);
        assert_eq!(d.map, states[0]);
        assert!(!d.is_dirty());
        // past the end stops at the end
        d.jump_to(99);
        assert_eq!(d.undo.len(), 5);
        assert_eq!(d.map, states[5]);
    }

    #[test]
    fn read_only_refuses_edits() {
        let mut d = doc();
        d.read_only = true;
        let before = d.map.clone();
        let ops = mover(&d, 3.0);
        d.commit("move", ops);
        let ops = mover(&d, 3.0);
        d.commit_coalesced("k", "move", ops);
        d.replace_with("x", &Svmap::new(128));
        assert_eq!(d.map, before);
        assert!(d.undo.is_empty() && d.redo.is_empty() && !d.is_dirty());
        assert!(d.refused >= 2, "the app turns the refusals into a toast");
        d.selection = vec![(SectionId::MobAreas, 0)];
        d.delete_selected();
        assert_eq!(d.map, before);
        assert!(d.undo.is_empty());
    }
}
