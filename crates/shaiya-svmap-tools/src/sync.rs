//! Sync from client: the server-side NPCs, portals, regions and bind areas of a map are
//! mirrors of what the client's `world/<id>.wld` holds, so a map whose client side was changed by another tool gets its server
//! side from there. This module turns a `.wld` into the four mirrored sections, compares them with a map, and builds the
//! undo operations for the rows the user ticked. "New from client map" is the same thing against an empty map.
//!
//! How each section is mirrored (all verified against the 77 original server maps; see `docs/formats/svmap.md`):
//!
//! * NPCs: record for record (type, type id, first point and angle, then the path points).
//! * portals: same index, same fields; the position is the centre (x, z) of the `.wld` box at its min y.
//! * regions: the `.wld` named areas, same boxes, `kind` = the area mode.
//! * binds: the `.wld` zone groups; a group with k boxes is one bind with k boxes, country and attr taken from its first box.
//!
//! Monster areas, section A, section B and the blocked cells are server-only and never touched.

use shaiya_clientdata::wld::{Wld, WldArea, WldNpc, WldPortal, ZoneGroup};
use shaiya_svmap::{Aabb, Bind, NpcEntry, NpcPoint, Portal, Region, SectionId, Svmap};

use crate::edit::{self, Op, Rec};

/// The four mirrored sections, in the order the dialog shows them.
pub const SECTIONS: [SectionId; 4] = [SectionId::Npcs, SectionId::Portals, SectionId::Regions, SectionId::Binds];

const POS_TOL: f32 = 0.01;
const ANGLE_TOL: f32 = 0.001;

/// What the `.wld` says the server side should be.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub npcs: Vec<NpcEntry>,
    pub portals: Vec<Portal>,
    pub regions: Vec<Region>,
    pub binds: Vec<Bind>,
}

impl Target {
    pub fn records(&self, s: SectionId) -> Vec<Rec> {
        match s {
            SectionId::Npcs => self.npcs.iter().cloned().map(Rec::Npc).collect(),
            SectionId::Portals => self.portals.iter().copied().map(Rec::Portal).collect(),
            SectionId::Regions => self.regions.iter().copied().map(Rec::Region).collect(),
            SectionId::Binds => self.binds.iter().cloned().map(Rec::Bind).collect(),
            _ => Vec::new(),
        }
    }
}

pub fn target_from_wld(w: &Wld) -> Target {
    Target {
        npcs: w.npcs.iter().map(npc_from_wld).collect(),
        portals: w.portals.iter().map(portal_from_wld).collect(),
        regions: w.areas.iter().map(region_from_wld).collect(),
        binds: w.zone_groups.iter().map(bind_from_wld).collect(),
    }
}

pub fn npc_from_wld(n: &WldNpc) -> NpcEntry {
    let mut points = vec![NpcPoint { pos: n.pos, dir: n.angle }];
    // the extra path points carry no angle in the .wld: they keep the first point's
    points.extend(n.extra.iter().map(|p| NpcPoint { pos: *p, dir: n.angle }));
    NpcEntry { kind: n.kind as u32, type_id: n.type_id as u32, points }
}

pub fn portal_from_wld(p: &WldPortal) -> Portal {
    Portal { pos: p.svmap_pos(), country: p.country as u32, level_min: p.level_min as u16, level_max: p.level_max as u16, to_map: p.dest_map as u32, to_pos: p.dest_pos }
}

/// The region tail (`b`, `c`, `d`) is the `.wld` area's trailing `i32`, split as one byte, one byte and a `u16`.
pub fn region_from_wld(a: &WldArea) -> Region {
    let e = a.extra as u32;
    Region { min: a.min, max: a.max, kind: a.mode as u32, b: (e & 0xFF) as u8, c: ((e >> 8) & 0xFF) as u8, d: (e >> 16) as u16 }
}

pub fn bind_from_wld(g: &ZoneGroup) -> Bind {
    let first = g.boxes.first();
    Bind {
        country: first.map_or(0, |b| b.country as u32),
        attr: first.map_or(0, |b| b.attr as u32),
        boxes: g.boxes.iter().map(|b| Aabb { min: b.min, max: b.max }).collect(),
    }
}

// ------------------------------------------------------------------------------------------------------------------ comparing

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn near3(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| near(*x, y, POS_TOL))
}

fn f3(v: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2])
}

/// Field-by-field differences between the map's record and the `.wld` record; empty when they agree (within 0.01 units and
/// 0.001 radians, the tolerances of the client rules).
pub fn differences(have: &Rec, want: &Rec) -> Vec<String> {
    let mut d = Vec::new();
    let mut check = |what: &str, same: bool, a: String, b: String| {
        if !same {
            d.push(format!("{what} {a} -> {b}"));
        }
    };
    match (have, want) {
        (Rec::Npc(a), Rec::Npc(b)) => {
            check("type", a.kind == b.kind, a.kind.to_string(), b.kind.to_string());
            check("type id", a.type_id == b.type_id, a.type_id.to_string(), b.type_id.to_string());
            check("points", a.points.len() == b.points.len(), a.points.len().to_string(), b.points.len().to_string());
            for (k, (p, q)) in a.points.iter().zip(&b.points).enumerate() {
                check(&format!("point {k}"), near3(p.pos, q.pos), f3(p.pos), f3(q.pos));
                // only the first point's angle is in the .wld
                if k == 0 {
                    check("angle", near(p.dir, q.dir, ANGLE_TOL), format!("{:.3}", p.dir), format!("{:.3}", q.dir));
                }
            }
        }
        (Rec::Portal(a), Rec::Portal(b)) => {
            check("position", near3(a.pos, b.pos), f3(a.pos), f3(b.pos));
            check("country", a.country == b.country, a.country.to_string(), b.country.to_string());
            check("level min", a.level_min == b.level_min, a.level_min.to_string(), b.level_min.to_string());
            check("level max", a.level_max == b.level_max, a.level_max.to_string(), b.level_max.to_string());
            check("to map", a.to_map == b.to_map, a.to_map.to_string(), b.to_map.to_string());
            check("destination", near3(a.to_pos, b.to_pos), f3(a.to_pos), f3(b.to_pos));
        }
        (Rec::Region(a), Rec::Region(b)) => {
            check("min", near3(a.min, b.min), f3(a.min), f3(b.min));
            check("max", near3(a.max, b.max), f3(a.max), f3(b.max));
            check("kind", a.kind == b.kind, a.kind.to_string(), b.kind.to_string());
            let tail = |r: &Region| format!("{}/{}/{}", r.b, r.c, r.d);
            check("tail", (a.b, a.c, a.d) == (b.b, b.c, b.d), tail(a), tail(b));
        }
        (Rec::Bind(a), Rec::Bind(b)) => {
            check("country", a.country == b.country, a.country.to_string(), b.country.to_string());
            check("attr", a.attr == b.attr, a.attr.to_string(), b.attr.to_string());
            check("boxes", a.boxes.len() == b.boxes.len(), a.boxes.len().to_string(), b.boxes.len().to_string());
            for (k, (p, q)) in a.boxes.iter().zip(&b.boxes).enumerate() {
                check(&format!("box {k}"), near3(p.min, q.min) && near3(p.max, q.max), format!("{}-{}", f3(p.min), f3(p.max)), format!("{}-{}", f3(q.min), f3(q.max)));
            }
        }
        _ => d.push("different kinds of record".to_string()),
    }
    d
}

/// One-line description of a record for the dialog.
pub fn summary(r: &Rec) -> String {
    match r {
        Rec::Npc(n) => match n.points.first() {
            Some(p) => format!("type {} id {} at ({:.1}, {:.1}){}", n.kind, n.type_id, p.pos[0], p.pos[2], if n.points.len() > 1 { format!(", {} points", n.points.len()) } else { String::new() }),
            None => format!("type {} id {} (no points)", n.kind, n.type_id),
        },
        Rec::Portal(p) => format!("at ({:.1}, {:.1}) to map {} ({:.1}, {:.1}) country {} lv {}-{}", p.pos[0], p.pos[2], p.to_map, p.to_pos[0], p.to_pos[2], p.country, p.level_min, p.level_max),
        Rec::Region(g) => format!("box ({:.1}, {:.1})-({:.1}, {:.1}) kind {}", g.min[0], g.min[2], g.max[0], g.max[2], g.kind),
        Rec::Bind(b) => format!("{} box(es), country {}, attr {}", b.boxes.len(), b.country, b.attr),
        _ => String::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// In the `.wld`, not in the map: it would be appended.
    Added,
    /// In the map, not in the `.wld`: it would be deleted.
    Removed,
    /// Both, with different fields: the map's record would be replaced.
    Changed,
}

/// Which side a row of the Sync window takes: the client's value into the svmap, or the svmap's into the client map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Take {
    #[default]
    Client,
    Server,
}

#[derive(Clone, Debug)]
pub struct SyncRow {
    pub section: SectionId,
    pub kind: RowKind,
    /// The map's record index (Changed, Removed); for Added the index in the `.wld` list.
    pub index: usize,
    /// The map's record (Changed, Removed).
    pub have: Option<Rec>,
    /// The `.wld` record (Added, Changed).
    pub want: Option<Rec>,
    pub text: String,
    pub checked: bool,
    /// The record's index in the client list (Changed, Added).
    pub want_index: Option<usize>,
    pub dir: Take,
}

#[derive(Clone, Debug, Default)]
pub struct SyncPlan {
    pub rows: Vec<SyncRow>,
    /// Records already in agreement, per section (same order as [`SECTIONS`]).
    pub same: [usize; 4],
    /// Record counts of the map and of the `.wld`, per section.
    pub have_count: [usize; 4],
    pub want_count: [usize; 4],
    /// The NPC pairs (map index, `.wld` index) when the two lists have other lengths and were paired by key.
    pub npc_matched: Option<Vec<(usize, usize)>>,
}

/// One record of the NPC list both sides end up with (see [`SyncPlan::npc_slots`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Both(usize, usize),
    Map(usize),
    Wld(usize),
}

fn slot(s: SectionId) -> usize {
    SECTIONS.iter().position(|x| *x == s).unwrap_or(0)
}

fn npc_key(n: &NpcEntry) -> Option<(u32, u32, i64, i64)> {
    let p = n.points.first()?;
    Some((n.kind, n.type_id, (p.pos[0] * 100.0).round() as i64, (p.pos[2] * 100.0).round() as i64))
}

/// Compares the four sections of `map` with `target`. Added and Changed rows start ticked; Removed rows (server-only records
/// the `.wld` does not have) start unticked, so a sync never deletes by default.
pub fn plan(map: &Svmap, target: &Target) -> SyncPlan {
    let mut p = SyncPlan::default();
    for s in SECTIONS {
        let have: Vec<Rec> = (0..edit::count(map, s)).filter_map(|i| edit::get(map, s, i)).collect();
        let want = target.records(s);
        let k = slot(s);
        p.have_count[k] = have.len();
        p.want_count[k] = want.len();
        // NPCs of a list with a different length are paired by (type, type id, position); everything else by index
        let pairs: Vec<(Option<usize>, Option<usize>)> = if s == SectionId::Npcs && have.len() != want.len() { pair_npcs(&have, &want) } else { pair_by_index(have.len(), want.len()) };
        if s == SectionId::Npcs && have.len() != want.len() {
            p.npc_matched = Some(pairs.iter().filter_map(|(h, w)| Some(((*h)?, (*w)?))).collect());
        }
        for (h, w) in pairs {
            match (h, w) {
                (Some(h), Some(w)) => {
                    let d = differences(&have[h], &want[w]);
                    if d.is_empty() {
                        p.same[k] += 1;
                    } else {
                        p.rows.push(SyncRow { section: s, kind: RowKind::Changed, index: h, have: Some(have[h].clone()), want: Some(want[w].clone()), text: d.join("; "), checked: true, want_index: Some(w), dir: Take::Client });
                    }
                }
                (None, Some(w)) => p.rows.push(SyncRow { section: s, kind: RowKind::Added, index: w, have: None, want: Some(want[w].clone()), text: summary(&want[w]), checked: true, want_index: Some(w), dir: Take::Client }),
                (Some(h), None) => p.rows.push(SyncRow { section: s, kind: RowKind::Removed, index: h, have: Some(have[h].clone()), want: None, text: summary(&have[h]), checked: false, want_index: None, dir: Take::Client }),
                (None, None) => {}
            }
        }
    }
    p
}

fn pair_by_index(nh: usize, nw: usize) -> Vec<(Option<usize>, Option<usize>)> {
    (0..nh.max(nw)).map(|i| ((i < nh).then_some(i), (i < nw).then_some(i))).collect()
}

fn pair_npcs(have: &[Rec], want: &[Rec]) -> Vec<(Option<usize>, Option<usize>)> {
    let key = |r: &Rec| match r {
        Rec::Npc(n) => npc_key(n),
        _ => None,
    };
    let mut used = vec![false; have.len()];
    let mut out = Vec::new();
    for (w, wr) in want.iter().enumerate() {
        let wk = key(wr);
        let found = (0..have.len()).find(|h| !used[*h] && wk.is_some() && key(&have[*h]) == wk);
        match found {
            Some(h) => {
                used[h] = true;
                out.push((Some(h), Some(w)));
            }
            None => out.push((None, Some(w))),
        }
    }
    for (h, u) in used.iter().enumerate() {
        if !u {
            out.push((Some(h), None));
        }
    }
    out
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows_of(&self, s: SectionId) -> impl Iterator<Item = (usize, &SyncRow)> {
        self.rows.iter().enumerate().filter(move |(_, r)| r.section == s)
    }

    pub fn checked_count(&self) -> usize {
        self.rows.iter().filter(|r| r.checked).count()
    }

    /// The undo operations for the ticked rows, in an order that applies cleanly: replacements first, then deletions from the
    /// back, then appends in `.wld` order.
    pub fn ops(&self, map: &Svmap) -> Vec<Op> {
        let mut ops = Vec::new();
        for s in SECTIONS {
            let rows: Vec<&SyncRow> = self.rows.iter().filter(|r| r.section == s && r.checked && r.dir == Take::Client).collect();
            for r in rows.iter().filter(|r| r.kind == RowKind::Changed) {
                if let (Some(b), Some(a)) = (&r.have, &r.want) {
                    ops.push(Op::replace(r.index, b.clone(), a.clone()));
                }
            }
            let mut removed: Vec<&SyncRow> = rows.iter().copied().filter(|r| r.kind == RowKind::Removed).collect();
            removed.sort_by_key(|r| std::cmp::Reverse(r.index));
            let n_removed = removed.len();
            for r in removed {
                if let Some(b) = &r.have {
                    ops.push(Op::remove(r.index, b.clone()));
                }
            }
            let mut at = edit::count(map, s) - n_removed.min(edit::count(map, s));
            let mut added: Vec<&SyncRow> = rows.iter().copied().filter(|r| r.kind == RowKind::Added).collect();
            added.sort_by_key(|r| r.index);
            if let Some(place) = (s == SectionId::Npcs).then(|| self.npc_insert_at(true)).flatten() {
                // at its place between the pairs, so the two lists end in the same order
                let mut v: Vec<(usize, &Rec)> = added.iter().filter_map(|r| Some((*place.get(&r.index)?, r.want.as_ref()?))).collect();
                v.sort_by_key(|(i, _)| *i);
                ops.extend(v.into_iter().map(|(i, a)| Op::insert(i, a.clone())));
                continue;
            }
            for r in added {
                if let Some(a) = &r.want {
                    ops.push(Op::insert(at, a.clone()));
                    at += 1;
                }
            }
        }
        ops
    }

    /// The order of the NPC lists after a sync, when they have other lengths and were paired by key: the pairs in `.wld` order,
    /// and between two pairs the map's own records, then the `.wld`'s. `None` when the lists are paired by index, or when the
    /// pairs cross (the two sides are in different orders anyway).
    fn npc_slots(&self) -> Option<Vec<Slot>> {
        let mut pairs = self.npc_matched.clone()?;
        let k = slot(SectionId::Npcs);
        let (nh, nw) = (self.have_count[k], self.want_count[k]);
        pairs.sort_by_key(|p| p.1);
        if pairs.windows(2).any(|x| x[0].0 >= x[1].0) {
            return None;
        }
        let mut out = Vec::with_capacity(nh + nw);
        let (mut ph, mut pw) = (0, 0);
        for &(h, w) in &pairs {
            out.extend((ph..h).map(Slot::Map));
            out.extend((pw..w).map(Slot::Wld));
            out.push(Slot::Both(h, w));
            (ph, pw) = (h + 1, w + 1);
        }
        out.extend((ph..nh).map(Slot::Map));
        out.extend((pw..nw).map(Slot::Wld));
        Some(out)
    }

    /// Where each NPC a side gains goes: its index in that side's list after the sync (`map_side`: the map's list, keyed by the
    /// `.wld` index of the Added row; else the client list, keyed by the map index of the Removed row). Appending instead would
    /// leave two lists of the same length in other orders, and the editor pairs NPCs by index then: an edit of one
    /// would change another record of the client map.
    fn npc_insert_at(&self, map_side: bool) -> Option<std::collections::HashMap<usize, usize>> {
        let slots = self.npc_slots()?;
        let takes = |kind: RowKind, i: usize, dir: Take| self.rows.iter().any(|r| r.section == SectionId::Npcs && r.kind == kind && r.index == i && r.checked && r.dir == dir);
        let mut at = std::collections::HashMap::new();
        let mut n = 0;
        for s in slots {
            let (present, gained) = match (s, map_side) {
                (Slot::Both(..), _) => (true, None),
                (Slot::Map(h), true) => (!takes(RowKind::Removed, h, Take::Client), None),
                (Slot::Wld(w), true) => {
                    let t = takes(RowKind::Added, w, Take::Client);
                    (t, t.then_some(w))
                }
                (Slot::Map(h), false) => {
                    let t = takes(RowKind::Removed, h, Take::Server);
                    (t, t.then_some(h))
                }
                (Slot::Wld(w), false) => (!takes(RowKind::Added, w, Take::Server), None),
            };
            if let Some(i) = gained {
                at.insert(i, n);
            }
            n += present as usize;
        }
        Some(at)
    }

    /// The ticked rows that change the client map.
    pub fn server_count(&self) -> usize {
        self.rows.iter().filter(|r| r.checked && r.dir == Take::Server).count()
    }

    /// The client ops of the ticked "take server" rows, made on the scratch lists `l`: changed records are replaced with the
    /// svmap's value, client records the svmap does not have go (from the back), svmap records the client does not have are
    /// appended in map order.
    pub fn client_ops(&self, l: &mut crate::pair::Lists) -> Vec<crate::pair::COp> {
        let mut ops = Vec::new();
        for s in SECTIONS {
            let rows: Vec<&SyncRow> = self.rows.iter().filter(|r| r.section == s && r.checked && r.dir == Take::Server).collect();
            for r in rows.iter().filter(|r| r.kind == RowKind::Changed) {
                if let (Some(w), Some(rec)) = (r.want_index, &r.have) {
                    ops.extend(crate::pair::take_server_replace(l, s, w, rec));
                }
            }
            let mut gone: Vec<usize> = rows.iter().filter(|r| r.kind == RowKind::Added).filter_map(|r| r.want_index).collect();
            gone.sort_by_key(|i| std::cmp::Reverse(*i));
            for w in gone {
                ops.extend(crate::pair::take_server_remove(l, s, w));
            }
            let mut add: Vec<&&SyncRow> = rows.iter().filter(|r| r.kind == RowKind::Removed).collect();
            add.sort_by_key(|r| r.index);
            if let Some(place) = (s == SectionId::Npcs).then(|| self.npc_insert_at(false)).flatten() {
                // at its place between the pairs (see `ops`)
                let mut v: Vec<(usize, &Rec)> = add.iter().filter_map(|r| Some((*place.get(&r.index)?, r.have.as_ref()?))).collect();
                v.sort_by_key(|(i, _)| *i);
                for (i, rec) in v {
                    ops.extend(crate::pair::take_server_insert(l, s, i, rec));
                }
                continue;
            }
            for r in add {
                if let Some(rec) = &r.have {
                    ops.extend(crate::pair::take_server_append(l, s, rec));
                }
            }
        }
        ops
    }

    pub fn set_section_dir(&mut self, s: SectionId, dir: Take) {
        for r in self.rows.iter_mut().filter(|r| r.section == s) {
            r.dir = dir;
        }
    }

    pub fn set_section(&mut self, s: SectionId, on: bool) {
        for r in self.rows.iter_mut().filter(|r| r.section == s) {
            r.checked = on;
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ one record

/// The undo operation that gives record `index` of `section` the `.wld` value of its counterpart ("apply this one record's
/// client value"): `Ok(None)` when the record already agrees (within the tolerances of [`differences`]), `Err` with the reason
/// when it cannot be done (a section that is not mirrored, an index past the end, or a record the `.wld` has no counterpart for).
/// Pairing is the one of [`plan`]: portals, regions and binds by index, NPCs by index when both lists have the same length and
/// by (type, type id, position) otherwise. Nothing is applied; the caller commits the op (see [`sync_record`]).
pub fn single_record_op(map: &Svmap, target: &Target, section: SectionId, index: usize) -> Result<Option<Op>, String> {
    if !SECTIONS.contains(&section) {
        return Err(format!("{} are not mirrored from the client's .wld", section.label()));
    }
    let have = edit::count(map, section);
    if index >= have {
        return Err(format!("{} #{index} does not exist (the map has {have})", section.label()));
    }
    let p = plan(map, target);
    match p.rows.iter().find(|r| r.section == section && r.index == index && r.kind != RowKind::Added) {
        Some(r) if r.kind == RowKind::Changed => match (&r.have, &r.want) {
            (Some(b), Some(a)) => Ok(Some(Op::replace(index, b.clone(), a.clone()))),
            _ => Err("internal: a changed row without both records".to_string()),
        },
        Some(_) => Err(format!("the .wld has no counterpart for {} #{index} (it has {} of them)", section.label(), target.records(section).len())),
        None => Ok(None),
    }
}

/// Applies [`single_record_op`] to an open map as one undo step labelled "sync <section> #i from client". Returns whether
/// anything changed.
pub fn sync_record(doc: &mut crate::doc::Doc, wld: &Wld, section: SectionId, index: usize) -> Result<bool, String> {
    match single_record_op(&doc.map, &target_from_wld(wld), section, index)? {
        Some(op) => {
            // the client map is where the value came from: only the svmap changes
            doc.commit_server_only(format!("sync {} #{index} from client", section.label()), vec![op]);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Label for the undo step.
pub fn step_label(plan: &SyncPlan) -> String {
    let n = plan.checked_count();
    format!("sync with client ({n} change{})", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;

    fn npc(kind: u32, id: u32, x: f32, z: f32) -> NpcEntry {
        NpcEntry { kind, type_id: id, points: vec![NpcPoint { pos: [x, 1.0, z], dir: 0.5 }] }
    }

    fn portal(x: f32, to: u32) -> Portal {
        Portal { pos: [x, 0.0, 5.0], country: 0, level_min: 1, level_max: 9, to_map: to, to_pos: [1.0, 2.0, 3.0] }
    }

    #[test]
    fn identical_sections_give_no_rows() {
        let mut m = Svmap::new(64);
        m.npcs.push(npc(1, 2, 10.0, 10.0));
        m.portals.push(portal(3.0, 2));
        let t = Target { npcs: m.npcs.clone(), portals: m.portals.clone(), ..Default::default() };
        let p = plan(&m, &t);
        assert!(p.is_empty());
        assert_eq!(p.same, [1, 1, 0, 0]);
        assert!(p.ops(&m).is_empty());
    }

    #[test]
    fn rows_ops_and_undo() {
        let mut m = Svmap::new(64);
        m.portals.push(portal(3.0, 2));
        m.portals.push(portal(8.0, 3));
        m.regions.push(Region { min: [1.0, 0.0, 1.0], max: [2.0, 1.0, 2.0], kind: 0, b: 0, c: 0, d: 0 });
        let mut t = Target::default();
        // portal 0 changed (destination), portal 1 same, a third portal is new; the region is gone from the .wld
        t.portals.push(portal(3.0, 9));
        t.portals.push(portal(8.0, 3));
        t.portals.push(portal(20.0, 4));
        let mut doc = Doc::new(1, Some(7), None, m.clone(), None);
        let mut p = plan(&doc.map, &t);
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[0].kind, RowKind::Changed);
        assert!(p.rows[0].text.contains("to map 2 -> 9"), "{}", p.rows[0].text);
        assert_eq!(p.rows[1].kind, RowKind::Added);
        assert_eq!((p.rows[2].kind, p.rows[2].checked), (RowKind::Removed, false), "server-only records are not deleted by default");
        let ops = p.ops(&doc.map);
        assert_eq!(ops.len(), 2);
        doc.commit(step_label(&p), ops);
        assert_eq!(doc.map.portals, t.portals);
        assert_eq!(doc.map.regions.len(), 1);
        assert_eq!(doc.undo.len(), 1);
        assert!(plan(&doc.map, &t).rows.iter().all(|r| r.section == SectionId::Regions));
        doc.undo();
        assert_eq!(doc.map, m);
        // ticking the removal too deletes the region, still one undo step
        p.rows[2].checked = true;
        doc.commit("sync", p.ops(&doc.map));
        assert!(doc.map.regions.is_empty());
        doc.undo();
        assert_eq!(doc.map, m);
        // unticking everything does nothing
        for r in &mut p.rows {
            r.checked = false;
        }
        assert!(p.ops(&doc.map).is_empty());
    }

    #[test]
    fn npcs_of_different_counts_are_paired_by_key() {
        let mut m = Svmap::new(64);
        m.npcs.push(npc(1, 1, 10.0, 10.0));
        m.npcs.push(npc(2, 5, 30.0, 30.0)); // only in the map
        m.npcs.push(npc(3, 3, 40.0, 40.0));
        let mut t = Target::default();
        t.npcs.push(npc(3, 3, 40.0, 40.0));
        t.npcs.push(npc(1, 1, 10.0, 10.0));
        let mut changed = npc(4, 4, 50.0, 50.0);
        changed.points[0].dir = 2.0;
        t.npcs.push(changed);
        t.npcs.push(npc(6, 6, 70.0, 70.0));
        let p = plan(&m, &t);
        assert_eq!(p.same[0], 2);
        let kinds: Vec<_> = p.rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Added, RowKind::Added, RowKind::Removed]);
        let ops = p.ops(&m);
        assert_eq!(ops.len(), 2, "only the Added rows are ticked");
        let mut doc = Doc::new(1, None, None, m, None);
        doc.commit("sync", ops);
        assert_eq!(doc.map.npcs.len(), 5);
        assert_eq!(doc.map.npcs[3].type_id, 4);
        assert_eq!(doc.map.npcs[4].kind, 6);
    }

    /// NPC lists of other lengths, paired by key: whatever each row takes, a sync that leaves both lists the same length leaves
    /// them in the same order (the editor pairs NPCs by index then, so an appended record would pair with the wrong one).
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn npc_sync_keeps_both_lists_in_the_same_order() {
        let key = |n: &NpcEntry| npc_key(n);
        let wnpc = |n: &NpcEntry| shaiya_clientdata::wld::WldNpc { kind: n.kind as i32, type_id: n.type_id as i32, pos: n.points[0].pos, angle: n.points[0].dir, extra: Vec::new() };
        let (a, b, c, d, e) = (npc(1, 1, 10.0, 10.0), npc(2, 2, 20.0, 20.0), npc(3, 3, 30.0, 30.0), npc(4, 4, 40.0, 40.0), npc(5, 5, 50.0, 50.0));
        // map A B D E, client A C D: B and E only in the map, C only in the client
        let cases: [(&[&NpcEntry], &[&NpcEntry]); 2] = [(&[&a, &b, &d, &e], &[&a, &c, &d]), (&[&a, &c, &d], &[&a, &b, &c, &d, &e])];
        for (mine, theirs) in cases {
            for dirs in 0..8u32 {
                let mut m = Svmap::new(64);
                m.npcs = mine.iter().map(|n| (*n).clone()).collect();
                let mut t = Target::default();
                t.npcs = theirs.iter().map(|n| (*n).clone()).collect();
                let mut l = crate::pair::Lists { npcs: theirs.iter().map(|n| wnpc(n)).collect(), ..Default::default() };
                let mut p = plan(&m, &t);
                for (k, r) in p.rows.iter_mut().enumerate() {
                    r.checked = true;
                    r.dir = if dirs >> (k % 3) & 1 == 1 { Take::Server } else { Take::Client };
                }
                for o in p.ops(&m) {
                    o.apply(&mut m);
                }
                p.client_ops(&mut l);
                assert_eq!(m.npcs.len(), l.npcs.len(), "every row was taken one way or the other");
                let ours: Vec<_> = m.npcs.iter().map(key).collect();
                let client: Vec<_> = l.npcs.iter().map(|w| key(&npc_from_wld(w))).collect();
                assert_eq!(ours, client, "the same order on both sides (directions {dirs:03b})");
            }
        }
    }

    #[test]
    fn region_tail_split() {
        let a = WldArea { min: [0.0; 3], max: [1.0; 3], rot_y: 0.0, name: String::new(), tga: String::new(), mode: 1, extra: 3 | (100 << 8) | (7 << 16) };
        let r = region_from_wld(&a);
        assert_eq!((r.kind, r.b, r.c, r.d), (1, 3, 100, 7));
    }

    #[test]
    fn one_record_is_synced_alone() {
        let mut m = Svmap::new(64);
        m.portals.push(portal(3.0, 2));
        m.portals.push(portal(8.0, 3));
        m.portals.push(portal(30.0, 4));
        let mut t = Target::default();
        t.portals.push(portal(3.0, 9)); // differs
        t.portals.push(portal(8.0, 3)); // same
        let mut doc = Doc::new(1, Some(7), None, m.clone(), None);
        // a differing record: the op replaces it with the .wld value and leaves its neighbours alone
        let op = single_record_op(&doc.map, &t, SectionId::Portals, 0).unwrap().expect("portal 0 differs");
        assert_eq!(op.index, 0);
        // an agreeing record needs nothing, a server-only record has no counterpart, bad requests say why
        assert_eq!(single_record_op(&doc.map, &t, SectionId::Portals, 1), Ok(None));
        assert!(single_record_op(&doc.map, &t, SectionId::Portals, 2).unwrap_err().contains("no counterpart"));
        assert!(single_record_op(&doc.map, &t, SectionId::Portals, 3).unwrap_err().contains("does not exist"));
        assert!(single_record_op(&doc.map, &t, SectionId::MobAreas, 0).unwrap_err().contains("not mirrored"));
        let w_portals = t.portals.clone();
        doc.commit("sync portals #0 from client", vec![op]);
        assert_eq!(doc.map.portals[0], w_portals[0]);
        assert_eq!(doc.map.portals[1], m.portals[1]);
        assert_eq!(doc.map.portals[2], m.portals[2]);
        assert_eq!(doc.undo.len(), 1);
        doc.undo();
        assert_eq!(doc.map, m);
    }

    #[test]
    fn one_npc_is_synced_by_key_when_the_lists_differ() {
        let mut m = Svmap::new(64);
        m.npcs.push(npc(2, 5, 30.0, 30.0)); // only in the map
        m.npcs.push(npc(3, 3, 40.0, 40.0));
        let mut wanted = npc(3, 3, 40.0, 40.0);
        wanted.points[0].dir = 2.5;
        let t = Target { npcs: vec![wanted.clone(), npc(1, 1, 10.0, 10.0), npc(6, 6, 7.0, 7.0)], ..Default::default() };
        let mut doc = Doc::new(1, None, None, m, None);
        assert!(single_record_op(&doc.map, &t, SectionId::Npcs, 0).unwrap_err().contains("no counterpart"));
        let op = single_record_op(&doc.map, &t, SectionId::Npcs, 1).unwrap().expect("the angle differs");
        assert_eq!(op.index, 1);
        doc.commit("one", vec![op]);
        assert_eq!(doc.map.npcs[1], wanted);
        assert_eq!(single_record_op(&doc.map, &t, SectionId::Npcs, 1), Ok(None));
    }

    #[test]
    fn sync_record_is_one_labelled_undo_step() {
        let Some((rules, dir)) = data() else { return };
        let w = rules.wld(2).unwrap();
        let orig = shaiya_svmap::read(&std::fs::read(dir.join("2.svmap")).unwrap()).unwrap();
        let mut doc = Doc::new(1, Some(2), None, orig.clone(), None);
        // NPC #181 of map 2 carries another angle than the .wld's
        assert_eq!(sync_record(&mut doc, &w, SectionId::Npcs, 181), Ok(true));
        assert_eq!(doc.undo.len(), 1);
        assert_eq!(doc.undo[0].label, "sync NPCs #181 from client");
        assert!((doc.map.npcs[181].points[0].dir - w.npcs[181].angle).abs() < 1e-6);
        // only that record changed
        for i in (0..orig.npcs.len()).filter(|i| *i != 181) {
            assert_eq!(doc.map.npcs[i], orig.npcs[i]);
        }
        assert_eq!(sync_record(&mut doc, &w, SectionId::Npcs, 181), Ok(false), "a second call finds it in agreement");
        // portals of map 2 already agree
        assert_eq!(sync_record(&mut doc, &w, SectionId::Portals, 0), Ok(false));
        doc.undo();
        assert_eq!(doc.map, orig);
    }

    fn data() -> Option<(crate::client_rules::ClientRules, std::path::PathBuf)> {
        let c = shaiya_testkit::env_client()?;
        let s = shaiya_testkit::env_var_path("SVMAP_ORIGINALS")?;
        if !(c.is_dir() && s.is_dir()) {
            return None;
        }
        Some((crate::client_rules::ClientRules::open(&c).ok()?, s))
    }

    #[test]
    fn originals_vs_wld_report() {
        let Some((rules, dir)) = data() else { return };
        let mut exact = [0usize; 4];
        let mut near_only = [0usize; 4];
        let mut total = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Some(w) = rules.wld(id) else { continue };
            let m = shaiya_svmap::read(&std::fs::read(&p).unwrap()).unwrap();
            let t = target_from_wld(&w);
            total += 1;
            let sec = [t.npcs == m.npcs, t.portals == m.portals, t.regions == m.regions, t.binds == m.binds];
            let plan = plan(&m, &t);
            for (k, s) in SECTIONS.iter().enumerate() {
                if sec[k] {
                    exact[k] += 1;
                } else if plan.rows.iter().all(|r| r.section != *s) {
                    near_only[k] += 1;
                    if k == 0 {
                        let bad = t.npcs.iter().zip(&m.npcs).position(|(a, b)| a != b);
                        if let Some(i) = bad {
                            eprintln!("map {id} NPC {i} not bit-equal: wld {:?} svmap {:?}", t.npcs[i], m.npcs[i]);
                        }
                    }
                } else {
                    let rows: Vec<String> = plan.rows.iter().filter(|r| r.section == *s).take(3).map(|r| format!("{:?} #{} {}", r.kind, r.index, r.text)).collect();
                    eprintln!("map {id} {}: {} rows, e.g. {rows:?}", s.label(), plan.rows.iter().filter(|r| r.section == *s).count());
                }
            }
        }
        eprintln!("{total} maps; byte-equal sections [npcs, portals, regions, binds] = {exact:?}; equal within tolerance but not bit-equal = {near_only:?}");
    }

    /// The bytes of one section of a map, written on its own in an otherwise empty map of the same size.
    fn section_bytes(m: &Svmap, s: SectionId) -> Vec<u8> {
        let mut e = Svmap::new(m.size);
        match s {
            SectionId::Npcs => e.npcs = m.npcs.clone(),
            SectionId::Portals => e.portals = m.portals.clone(),
            SectionId::Regions => e.regions = m.regions.clone(),
            SectionId::Binds => e.binds = m.binds.clone(),
            SectionId::MobAreas => e.mob_areas = m.mob_areas.clone(),
            _ => {}
        }
        shaiya_svmap::write(&e)
    }

    /// Acceptance: "new from client map 2" plus map 2's monster areas from the original has no errors and the same NPC,
    /// portal, region and bind sections as the original file.
    #[test]
    fn new_from_client_map_2_matches_the_original() {
        let Some((rules, dir)) = data() else { return };
        let orig = shaiya_svmap::read(&std::fs::read(dir.join("2.svmap")).unwrap()).unwrap();
        let w = rules.wld(2).unwrap();
        let size = w.field_size().unwrap();
        assert_eq!(size, orig.size);
        let mut doc = Doc::new_unsaved(1, Some(2), Svmap::new(size));
        assert!(doc.is_dirty());
        let p = plan(&doc.map, &target_from_wld(&w));
        assert!(p.rows.iter().all(|r| r.kind == RowKind::Added));
        doc.commit(step_label(&p), p.ops(&doc.map));
        // copy the monster areas from the original
        let ops: Vec<Op> = orig.mob_areas.iter().enumerate().map(|(i, a)| Op::insert(i, Rec::Mob(a.clone()))).collect();
        doc.commit("copy monster areas", ops);
        // NPC #181 is stored with another angle in the original than in the .wld (3.477 against 3.449, found by the client rules);
        // no tool can know that.
        // Path points carry their own heading in the svmap (random-looking values the .wld does not hold), so NPCs with a path
        // differ in those angles only.
        assert_eq!(doc.map.npcs.len(), orig.npcs.len());
        let mut compared = doc.map.clone();
        let mut path_angles = 0;
        let mut other = Vec::new();
        for i in 0..orig.npcs.len() {
            let (a, b) = (&doc.map.npcs[i], &orig.npcs[i]);
            if a == b {
                continue;
            }
            let same_but_path_angles = a.kind == b.kind && a.type_id == b.type_id && a.points.len() == b.points.len() && a.points[0] == b.points[0] && a.points.iter().zip(&b.points).all(|(p, q)| p.pos == q.pos);
            if same_but_path_angles {
                path_angles += 1;
            } else {
                other.push(i);
            }
            compared.npcs[i] = b.clone();
        }
        assert_eq!(other, vec![181]);
        {
            let (a, b) = (&doc.map.npcs[181], &orig.npcs[181]);
            assert_eq!((a.kind, a.type_id, a.points.len(), a.points[0].pos), (b.kind, b.type_id, b.points.len(), b.points[0].pos));
            assert!((a.points[0].dir - b.points[0].dir).abs() < 0.05);
        }
        eprintln!("map 2 from the client: {path_angles} NPCs differ from the original only in path-point angles, NPC #181 differs in its angle (3.449 against 3.477)");
        let mut sections = Vec::new();
        for s in [SectionId::Npcs, SectionId::Portals, SectionId::Regions, SectionId::Binds, SectionId::MobAreas] {
            let (a, b) = (section_bytes(&compared, s), section_bytes(&orig, s));
            sections.push(format!("{} {} bytes {}", s.label(), a.len(), if a == b { "equal" } else { "DIFFERENT" }));
            assert_eq!(a, b, "{} differs from the original", s.label());
        }
        eprintln!("map 2 from the client: {}", sections.join("; "));
        let mut errors: Vec<_> = shaiya_svmap::validate(&doc.map).into_iter().filter(|f| f.severity == shaiya_svmap::Severity::Error).collect();
        errors.extend(rules.check(Some(2), &doc.map).into_iter().filter(|f| f.severity == shaiya_svmap::Severity::Error));
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|f| f.message.clone()).collect::<Vec<_>>());
        // the file written from it reads back to the same map; a second sync finds nothing to do
        assert_eq!(shaiya_svmap::read(&shaiya_svmap::write(&doc.map)).unwrap(), doc.map);
        assert!(plan(&doc.map, &target_from_wld(&w)).is_empty());
        // and the sync is one undo step
        assert_eq!(doc.undo.len(), 2);
    }
}
