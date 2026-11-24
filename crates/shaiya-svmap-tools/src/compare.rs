//! Compare and revert. The Changes tab compares the open map with the last save, with the
//! `<file>.bak` the first save made, or with the same map in the reference folder; every difference can be shown, drawn over the
//! map (the compare layer) and reverted one record, one section or the blocked cells at a time. This module holds the reference
//! loading, the cached difference rows, the overlay geometry and the revert ops; the window code is in `app.rs`.
//!
//! The rows come from an order-aware alignment (a longest common subsequence of the two record lists), not from the library's
//! matching by value: a record that moved up or down the list is a change, and reverting rows one by one always ends with the
//! reference's records in the reference's order, which the library's diff (it ignores order inside a stretch) cannot promise.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shaiya_svmap::diff::DiffKind;
use shaiya_svmap::{Run, SectionId, Svmap};

use crate::doc::{stamp_of, Doc, Stamp};
use crate::edit::{self, Op, Rec};
use crate::keys::CompareAgainst;

/// `<file>.bak`: the file as it was before the editor's first save.
pub fn backup_path(file: &Path) -> PathBuf {
    let mut n = file.file_name().unwrap_or_default().to_os_string();
    n.push(".bak");
    file.with_file_name(n)
}

/// The same map in the reference folder.
pub fn reference_path(folder: &Path, map_id: u32) -> PathBuf {
    folder.join(format!("{map_id}.svmap"))
}

/// Why a reference cannot be used, or the file it reads (`None` = the document's own last save). `reference_folder` is the folder of
/// the reference or of the other folder, whichever `against` names.
pub fn source_of(doc: &Doc, against: CompareAgainst, reference_folder: Option<&Path>) -> Result<Option<PathBuf>, &'static str> {
    match against {
        CompareAgainst::Saved => Ok(None),
        CompareAgainst::Backup => match &doc.path {
            None => Err("this map has no file yet"),
            Some(p) => {
                let b = backup_path(p);
                if b.is_file() {
                    Ok(Some(b))
                } else {
                    Err("no .bak yet (the first save keeps one)")
                }
            }
        },
        CompareAgainst::Reference => {
            let Some(folder) = reference_folder else { return Err("no reference folder is set (Help > Setup...)") };
            let Some(id) = doc.map_id else { return Err("this map has no number") };
            let p = reference_path(folder, id);
            if p.is_file() {
                Ok(Some(p))
            } else {
                Err("the reference folder has no copy of this map")
            }
        }
        CompareAgainst::Other => {
            let Some(folder) = reference_folder else { return Err("no other folder is set (Tools > Maps > Compare folders...)") };
            let Some(id) = doc.map_id else { return Err("this map has no number") };
            let p = reference_path(folder, id);
            if p.is_file() {
                Ok(Some(p))
            } else {
                Err("the other folder has no copy of this map")
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ rows

/// One difference in a list section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: DiffKind,
    pub section: SectionId,
    /// The record in the reference (changed and removed rows).
    pub ref_index: Option<usize>,
    /// The record in the open map (changed and added rows); for a removed row, where the record goes back in.
    pub cur_index: usize,
}

/// Aligns two lists of record keys: returns the matched pairs `(a, b)` in order (a longest common subsequence). Both ends that
/// are equal are taken first; a middle too large for the table is left to the caller as "all different".
fn lcs_pairs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let (mut ea, mut eb) = (a.len(), b.len());
    while ea > start && eb > start && a[ea - 1] == b[eb - 1] {
        ea -= 1;
        eb -= 1;
    }
    let mut pairs: Vec<(usize, usize)> = (0..start).map(|i| (i, i)).collect();
    let (n, m) = (ea - start, eb - start);
    if n > 0 && m > 0 && n.saturating_mul(m) <= 4_000_000 {
        // t[i][j] = length of the LCS of a[start+i..ea] and b[start+j..eb]
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * w];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if a[start + i] == b[start + j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a[start + i] == b[start + j] {
                pairs.push((start + i, start + j));
                i += 1;
                j += 1;
            } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    for k in 0..(a.len() - ea) {
        pairs.push((ea + k, eb + k));
    }
    pairs
}

/// The rows of one list section: between two matched records, the records left over on both sides are paired in order as
/// "changed" and the surplus is "removed" (only the reference has it) or "added" (only the open map has it).
pub fn section_rows(reference: &Svmap, current: &Svmap, s: SectionId) -> Vec<Row> {
    let key = |m: &Svmap| edit::records(m, s).iter().map(|r| format!("{r:?}")).collect::<Vec<_>>();
    let (a, b) = (key(reference), key(current));
    let pairs = lcs_pairs(&a, &b);
    let mut rows = Vec::new();
    let (mut i0, mut j0) = (0, 0);
    let segment = |i0: usize, j0: usize, i1: usize, j1: usize, rows: &mut Vec<Row>| {
        let (il, jl) = (i1 - i0, j1 - j0);
        for k in 0..il.min(jl) {
            rows.push(Row { kind: DiffKind::Changed, section: s, ref_index: Some(i0 + k), cur_index: j0 + k });
        }
        for k in jl..il {
            rows.push(Row { kind: DiffKind::Removed, section: s, ref_index: Some(i0 + k), cur_index: j1 });
        }
        for k in il..jl {
            rows.push(Row { kind: DiffKind::Added, section: s, ref_index: None, cur_index: j0 + k });
        }
    };
    for (pi, pj) in pairs {
        segment(i0, j0, pi, pj, &mut rows);
        i0 = pi + 1;
        j0 = pj + 1;
    }
    segment(i0, j0, a.len(), b.len(), &mut rows);
    rows
}

pub const LIST_SECTIONS: [SectionId; 7] = [SectionId::SectionA, SectionId::SectionB, SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions];

/// Every difference of the list sections, in file order.
pub fn rows(reference: &Svmap, current: &Svmap) -> Vec<Row> {
    LIST_SECTIONS.iter().flat_map(|s| section_rows(reference, current, *s)).collect()
}

// ------------------------------------------------------------------------------------------------------------------ overlay

type Bounds = (f32, f32, f32, f32);

/// One difference as the viewport draws it.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: DiffKind,
    /// Where the record is in the reference (changed and removed records).
    pub ref_bounds: Option<Bounds>,
    /// Where it is in the open map (changed and added records).
    pub cur_bounds: Option<Bounds>,
    pub tip: String,
}

#[derive(Clone, Debug, Default)]
pub struct Overlay {
    pub items: Vec<Item>,
}

fn centre(b: Bounds) -> [f32; 2] {
    [(b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5]
}

/// What moved between two versions of a record: `x 32.00 → 33.00, z 0.00 → 2.00`, or "fields differ" when the position is the same.
pub fn move_text(rr: &Rec, cr: &Rec) -> String {
    let (ra, ca) = (crate::edit::anchor_of(rr), crate::edit::anchor_of(cr));
    let mut parts = Vec::new();
    if (ra[0] - ca[0]).abs() > 0.004 {
        parts.push(format!("x {:.2} → {:.2}", ra[0], ca[0]));
    }
    if (ra[1] - ca[1]).abs() > 0.004 {
        parts.push(format!("z {:.2} → {:.2}", ra[1], ca[1]));
    }
    if parts.is_empty() {
        "fields differ (the position is the same)".to_string()
    } else {
        parts.join(", ")
    }
}

fn tip_for(kind: DiffKind, against: CompareAgainst, r: Option<&Rec>, c: Option<&Rec>) -> String {
    let what = against.short();
    match (kind, r, c) {
        (DiffKind::Changed, Some(rr), Some(cr)) => format!("changed vs {what}: {}", move_text(rr, cr)),
        (DiffKind::Removed, ..) => format!("not in the map any more: it is in the {what}"),
        (DiffKind::Added, ..) => format!("new: not in the {what}"),
        _ => format!("changed vs {what}"),
    }
}

pub fn overlay(reference: &Svmap, current: &Svmap, rows: &[Row], against: CompareAgainst) -> Overlay {
    let mut items = Vec::new();
    for r in rows {
        let rr = match r.kind {
            DiffKind::Added => None,
            _ => r.ref_index.and_then(|i| edit::get(reference, r.section, i)),
        };
        let cr = match r.kind {
            DiffKind::Removed => None,
            _ => edit::get(current, r.section, r.cur_index),
        };
        let tip = tip_for(r.kind, against, rr.as_ref(), cr.as_ref());
        items.push(Item { kind: r.kind, ref_bounds: rr.as_ref().map(edit::bounds_xz), cur_bounds: cr.as_ref().map(edit::bounds_xz), tip });
    }
    Overlay { items }
}

/// Line from the reference position to the current one (changed records), in world x, z.
pub fn move_line(i: &Item) -> Option<([f32; 2], [f32; 2])> {
    match (i.ref_bounds, i.cur_bounds) {
        (Some(r), Some(c)) => {
            let (a, b) = (centre(r), centre(c));
            ((a[0] - b[0]).abs() > 0.01 || (a[1] - b[1]).abs() > 0.01).then_some((a, b))
        }
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------------------------ the cache

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    against: CompareAgainst,
    edit_gen: u64,
    base_gen: u64,
    ref_stamp: Option<Stamp>,
}

/// The cached comparison of one tab.
#[derive(Default)]
pub struct Cmp {
    key: Option<Key>,
    /// The reference map (None for "last save": that is the document's own `base`).
    pub reference: Option<Arc<Svmap>>,
    pub error: Option<String>,
    pub rows: Vec<Row>,
    pub overlay: Overlay,
    /// Blocked cells that differ, as runs of the cell index (`z * size + x`); `None` when the reference has another size.
    pub cell_runs: Option<Vec<Run>>,
    pub size_differs: Option<(u32, u32)>,
    /// Moves whenever the result was recomputed (the compare layer's cell texture is rebuilt for it).
    pub rev: u64,
}

impl Cmp {
    /// Brings the comparison up to date: reloads the reference file when it changed (or `against` did) and recomputes the
    /// rows when the map, the last save or the reference changed. Cheap when nothing did.
    pub fn update(&mut self, doc: &Doc, against: CompareAgainst, reference_folder: Option<&Path>) {
        let source = source_of(doc, against, reference_folder);
        let ref_stamp = match &source {
            Ok(Some(p)) => stamp_of(p),
            _ => None,
        };
        let key = Key { against, edit_gen: doc.edit_gen, base_gen: doc.base_gen, ref_stamp };
        if self.key == Some(key) {
            return;
        }
        let ref_changed = self.key.is_none_or(|k| k.against != against || k.ref_stamp != ref_stamp);
        self.key = Some(key);
        if ref_changed {
            self.error = None;
            self.reference = None;
            match &source {
                Ok(Some(p)) => match std::fs::read(p).map_err(|e| e.to_string()).and_then(|b| shaiya_svmap::read(&b).map_err(|e| e.to_string())) {
                    Ok(m) => self.reference = Some(Arc::new(m)),
                    Err(e) => self.error = Some(format!("{}: {e}", p.display())),
                },
                Ok(None) => {}
                Err(why) => self.error = Some(why.to_string()),
            }
        }
        let computed = self.reference_map(doc).map(|r| {
            let all = rows(r, &doc.map);
            (overlay(r, &doc.map, &all, against), all, r.blocked.diff_runs(&doc.map.blocked), (r.size != doc.map.size).then_some((r.size, doc.map.size)))
        });
        match computed {
            Some((o, all, runs, size)) => {
                self.overlay = o;
                self.rows = all;
                self.cell_runs = runs;
                self.size_differs = size;
            }
            None => {
                self.rows.clear();
                self.overlay = Overlay::default();
                self.cell_runs = None;
                self.size_differs = None;
            }
        }
        self.rev += 1;
    }

    /// The map the rows are against: the loaded reference, or the document's last save.
    pub fn reference_map<'a>(&'a self, doc: &'a Doc) -> Option<&'a Svmap> {
        match self.key.map(|k| k.against) {
            Some(CompareAgainst::Saved) | None => Some(&doc.base),
            Some(_) => self.reference.as_deref(),
        }
    }

    /// Number of blocked cells that differ.
    pub fn cells(&self) -> u64 {
        self.cell_runs.as_ref().map_or(0, |r| r.iter().map(|x| x.len).sum())
    }

    /// Whether anything differs at all (list rows, cells or size).
    pub fn any(&self) -> bool {
        !self.rows.is_empty() || self.cells() > 0 || self.size_differs.is_some()
    }
}

// ------------------------------------------------------------------------------------------------------------------ revert

/// The undo op that puts one difference back the way the reference has it, and a label for the step. Changed: the open map's
/// record is replaced by the reference's; removed: the reference's record is inserted again where it belongs; added: the record
/// is removed.
pub fn revert_row(reference: &Svmap, current: &Svmap, r: &Row) -> Option<(String, Op)> {
    let label = |what: &str| format!("revert {what} {} #{}", r.section.label(), r.ref_index.unwrap_or(r.cur_index));
    match r.kind {
        DiffKind::Changed => Some((label("changed"), Op::replace(r.cur_index, edit::get(current, r.section, r.cur_index)?, edit::get(reference, r.section, r.ref_index?)?))),
        DiffKind::Removed => Some((label("removed"), Op::insert(r.cur_index.min(edit::count(current, r.section)), edit::get(reference, r.section, r.ref_index?)?))),
        DiffKind::Added => Some((label("added"), Op::remove(r.cur_index, edit::get(current, r.section, r.cur_index)?))),
    }
}

/// The ops that make a whole section equal to the reference's (exactly, order included).
pub fn revert_section(reference: &Svmap, current: &Svmap, s: SectionId) -> Vec<Op> {
    edit::list_ops(&edit::records(current, s), &edit::records(reference, s))
}

/// Whether the ops add or remove portals or regions: the client mirrors their count and order, so the editor asks first.
pub fn changes_portal_or_region_count(ops: &[Op]) -> bool {
    ops.iter().any(|o| matches!(o.kind, edit::OpKind::Insert(_) | edit::OpKind::Remove(_)) && matches!(o.section(), SectionId::Portals | SectionId::Regions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use shaiya_svmap::{MobArea, MobEntry, NpcEntry, NpcPoint, Portal};

    fn area(x: f32) -> MobArea {
        MobArea { min: [x, 0.0, 0.0], max: [x + 4.0, 2.0, 4.0], mobs: vec![MobEntry { mob: 1, count: 1 }] }
    }

    fn base() -> Svmap {
        let mut m = Svmap::new(64);
        m.mob_areas = (0..6).map(|i| area(i as f32 * 8.0)).collect();
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [5.0, 0.0, 5.0], dir: 0.0 }] });
        m.portals.push(Portal { pos: [1.0, 0.0, 1.0], country: 0, level_min: 0, level_max: 0, to_map: 2, to_pos: [3.0, 0.0, 3.0] });
        m
    }

    /// Reverts the first row over and over until there is none; every revert is one undo step on the document.
    fn revert_all(reference: &Svmap, d: &mut Doc) -> usize {
        let mut n = 0;
        loop {
            let all = rows(reference, &d.map);
            let Some(r) = all.first() else { break };
            let (label, op) = revert_row(reference, &d.map, r).expect("a list row reverts");
            let before = d.undo.len();
            d.commit(label, vec![op]);
            assert_eq!(d.undo.len(), before + 1, "one step per revert");
            n += 1;
            assert!(n < 100, "the reverts converge");
        }
        n
    }

    #[test]
    fn an_order_aware_alignment_pairs_the_right_records() {
        // delete area 1 and move area 4: the move is a change of 4, not of 1
        let reference = base();
        let mut cur = base();
        cur.mob_areas.remove(1);
        cur.mob_areas[3].min[0] += 1.0; // was index 4
        let r = section_rows(&reference, &cur, SectionId::MobAreas);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.contains(&Row { kind: DiffKind::Removed, section: SectionId::MobAreas, ref_index: Some(1), cur_index: 1 }), "{r:?}");
        assert!(r.contains(&Row { kind: DiffKind::Changed, section: SectionId::MobAreas, ref_index: Some(4), cur_index: 3 }), "{r:?}");
        // equal lists have no rows; a swap is two changes
        assert!(rows(&reference, &base()).is_empty());
        let mut swapped = base();
        swapped.mob_areas.swap(2, 3);
        assert_eq!(section_rows(&reference, &swapped, SectionId::MobAreas).len(), 2);
    }

    #[test]
    fn every_kind_of_difference_reverts_to_the_reference_bytes() {
        let reference = base();
        let mut d = Doc::new(1, Some(1), None, base(), None);
        // changed (a moved area and a changed portal), removed (the last area), added (an NPC in front)
        let before = edit::get(&d.map, SectionId::MobAreas, 4).unwrap();
        let mut after = before.clone();
        edit::translate(&mut after, 1.0, 2.0);
        d.commit("move", vec![Op::replace(4, before, after)]);
        let before = edit::get(&d.map, SectionId::Portals, 0).unwrap();
        let Rec::Portal(mut p) = before.clone() else { panic!() };
        p.to_map = 9;
        d.commit("portal", vec![Op::replace(0, before, Rec::Portal(p))]);
        d.selection = vec![(SectionId::MobAreas, 5)];
        d.delete_selected();
        d.commit("add npc", vec![Op::insert(0, Rec::Npc(NpcEntry { kind: 2, type_id: 3, points: vec![NpcPoint { pos: [9.0, 0.0, 9.0], dir: 1.0 }] }))]);
        let all = rows(&reference, &d.map);
        assert_eq!(all.len(), 4, "{all:?}");
        let o = overlay(&reference, &d.map, &all, CompareAgainst::Saved);
        assert!(o.items.iter().any(|i| i.kind == DiffKind::Changed && i.tip.contains("x 32.00 → 33.00")), "{:?}", o.items.iter().map(|i| &i.tip).collect::<Vec<_>>());
        assert!(o.items.iter().any(|i| i.kind == DiffKind::Removed) && o.items.iter().any(|i| i.kind == DiffKind::Added));
        assert!(o.items.iter().filter_map(move_line).count() >= 1, "a moved record has a line");
        let steps_before = d.undo.len();
        assert_eq!(revert_all(&reference, &mut d), 4);
        assert_eq!(d.undo.len(), steps_before + 4);
        assert_eq!(shaiya_svmap::write(&d.map), shaiya_svmap::write(&reference));
        // undoing a revert brings the edit back
        d.undo();
        assert!(!rows(&reference, &d.map).is_empty());
    }

    #[test]
    fn reverting_in_any_order_ends_in_the_reference_order() {
        let reference = base();
        let mut d = Doc::new(1, Some(1), None, base(), None);
        d.selection = vec![(SectionId::MobAreas, 1), (SectionId::MobAreas, 2)];
        d.delete_selected(); // two neighbours removed
        // revert the second removed row first
        let all = rows(&reference, &d.map);
        assert_eq!(all.len(), 2);
        let (l, op) = revert_row(&reference, &d.map, &all[1]).unwrap();
        d.commit(l, vec![op]);
        assert_eq!(revert_all(&reference, &mut d), 1);
        assert_eq!(shaiya_svmap::write(&d.map), shaiya_svmap::write(&reference));
    }

    #[test]
    fn reverting_a_section_and_the_cells() {
        let reference = base();
        let mut d = Doc::new(1, Some(1), None, base(), None);
        d.selection = vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 3)];
        d.delete_selected();
        d.commit("add", vec![Op::insert(0, Rec::Mob(area(50.0))), Op::insert(1, Rec::Mob(area(55.0)))]);
        let mut g = d.map.blocked.clone();
        g.fill_rect(3, 3, 9, 8, true);
        d.commit_blocked("paint", &g);
        let mut c = Cmp::default();
        c.update(&d, CompareAgainst::Saved, None);
        assert_eq!(c.cells(), 30);
        assert!(c.any());
        let ops = revert_section(&reference, &d.map, SectionId::MobAreas);
        assert!(!changes_portal_or_region_count(&ops), "monster areas are not mirrored by the client");
        d.commit("revert section", ops);
        assert_eq!(d.map.mob_areas, reference.mob_areas);
        assert_eq!(d.undo.len(), 4);
        assert_eq!(d.commit_blocked("revert cells", &reference.blocked), 30);
        assert_eq!(shaiya_svmap::write(&d.map), shaiya_svmap::write(&reference));
        // a portal section revert that removes a portal is flagged
        let mut with_extra = reference.clone();
        with_extra.portals.push(reference.portals[0]);
        let ops = revert_section(&reference, &with_extra, SectionId::Portals);
        assert!(changes_portal_or_region_count(&ops));
    }

    #[test]
    fn the_reference_is_read_once_and_follows_the_file() {
        let dir = TempDir::new("compare-ref");
        let maps = dir.join("maps");
        let refs = dir.join("ref");
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::create_dir_all(&refs).unwrap();
        let file = maps.join("5.svmap");
        std::fs::write(&file, shaiya_svmap::write(&base())).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        // no .bak, no reference: the reasons
        assert!(source_of(&d, CompareAgainst::Backup, None).unwrap_err().contains("no .bak"));
        assert!(source_of(&d, CompareAgainst::Reference, None).unwrap_err().contains("no reference folder"));
        assert!(source_of(&d, CompareAgainst::Reference, Some(&refs)).unwrap_err().contains("no copy"));
        assert_eq!(source_of(&d, CompareAgainst::Saved, None), Ok(None));
        // the first save makes the .bak
        d.commit("edit", vec![Op::remove(0, edit::get(&d.map, SectionId::MobAreas, 0).unwrap())]);
        d.save(None).unwrap();
        d.commit("edit again", vec![Op::remove(0, edit::get(&d.map, SectionId::MobAreas, 0).unwrap())]);
        let mut c = Cmp::default();
        c.update(&d, CompareAgainst::Backup, None);
        assert!(c.error.is_none(), "{:?}", c.error);
        assert_eq!(c.rows.len(), 2, "two areas are missing against the original: {:?}", c.rows);
        let rev = c.rev;
        c.update(&d, CompareAgainst::Backup, None);
        assert_eq!(c.rev, rev, "nothing changed: nothing recomputed");
        // against the last save: one removal
        c.update(&d, CompareAgainst::Saved, None);
        assert_eq!(c.rows.len(), 1);
        // a reference copy
        std::fs::write(refs.join("5.svmap"), shaiya_svmap::write(&base())).unwrap();
        c.update(&d, CompareAgainst::Reference, Some(&refs));
        assert_eq!(c.rows.len(), 2);
        // an unreadable reference gives an error and no differences
        std::fs::write(refs.join("5.svmap"), b"junk").unwrap();
        c.update(&d, CompareAgainst::Reference, Some(&refs));
        assert!(c.error.is_some() && c.rows.is_empty() && !c.any());
        assert!(backup_path(&file).ends_with("5.svmap.bak"));
    }

    #[test]
    fn compare_revert_roundtrip() {
        // `[SVMAP_ORIGINALS]`: five scripted edits on a copy of map 1 compared with the originals folder
        let Some(dir) = shaiya_testkit::env_var_path("SVMAP_ORIGINALS") else {
            return;
        };
        let folder = dir;
        let orig = std::fs::read(folder.join("1.svmap")).unwrap();
        let reference = shaiya_svmap::read(&orig).unwrap();
        let mut d = Doc::new(1, Some(1), None, reference.clone(), None);
        let before = edit::get(&d.map, SectionId::MobAreas, 10).unwrap();
        let mut after = before.clone();
        edit::translate(&mut after, 12.5, -7.25);
        d.commit("move area", vec![Op::replace(10, before, after)]);
        let before = edit::get(&d.map, SectionId::Portals, 0).unwrap();
        let mut after = before.clone();
        if let Rec::Portal(p) = &mut after {
            p.to_map = 18;
            p.to_pos = [100.5, 2.0, 200.25];
        }
        d.commit("portal destination", vec![Op::replace(0, before, after)]);
        let before = edit::get(&d.map, SectionId::Npcs, 3).unwrap();
        let mut after = before.clone();
        if let Rec::Npc(n) = &mut after {
            let last = *n.points.last().unwrap();
            n.points.push(NpcPoint { pos: [last.pos[0] + 3.0, last.pos[1], last.pos[2]], dir: 1.0 });
        }
        d.commit("add path point", vec![Op::replace(3, before, after)]);
        d.commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
        d.selection = vec![(SectionId::MobAreas, 5), (SectionId::Npcs, 7)];
        d.delete_selected();
        assert_eq!(d.undo.len(), 5);
        // against the reference folder = the originals, through the tab's cache as the Changes tab does it
        let mut c = Cmp::default();
        c.update(&d, CompareAgainst::Reference, Some(&folder));
        assert!(c.error.is_none(), "{:?}", c.error);
        eprintln!("compare_revert_roundtrip: 5 edits give {} difference rows", c.rows.len());
        // five steps, six rows: the last step deletes two records
        assert_eq!(c.rows.len(), 6, "moved area, portal, NPC path, added B, removed area, removed NPC: {:?}", c.rows);
        assert_eq!(c.cells(), 0);
        let mut n = 0;
        loop {
            c.update(&d, CompareAgainst::Reference, Some(&folder));
            let Some(r) = c.rows.first().copied() else { break };
            let (label, op) = revert_row(c.reference_map(&d).unwrap(), &d.map, &r).expect("a list row reverts");
            d.commit(label, vec![op]);
            n += 1;
            assert!(n <= 6, "one revert per row");
        }
        assert_eq!(n, 6);
        assert_eq!(d.undo.len(), 11, "one undo step per revert");
        assert!(!c.any());
        assert_eq!(shaiya_svmap::write(&d.map), orig, "reverting every row gives the original bytes");
    }
}
