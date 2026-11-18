//! Placements: the buildings, shapes, trees and dungeon pieces a client
//! `.wld` places. Everything here is plain data: the placement operations build [`COp`]s against the tab's lossless client model,
//! the document applies them as one undo step, and the picking maths works on rays, so the 2D view (a ray straight down) and the
//! 3D view (the camera ray) share it.
//!
//! The rules that keep the file safe:
//!
//! * A placement points at a name **by index**. No operation ever removes or reorders a name: delete removes the placement only,
//!   add appends the model's name when the list lacks it (and undo takes that append back), so no index of another placement shifts.
//! * Rotation turns `dir` and `up` about the world Y axis together and re-normalises, so both stay unit length and perpendicular.
//! * Nothing here writes a file. The client half is saved through the staging folder like the other client edits.

use std::collections::HashMap;

use shaiya_clientdata::smod::placement_basis;
use shaiya_clientdata::wld::Placement;
use shaiya_clientdata::wld_raw::{name_text, Codepage, Name256, RawPlaced, WldRaw};
use shaiya_clientdata::ClientSource;

use crate::math3d::V3;
use crate::pair::{CKind, COp};

/// The four lists the editor changes. Grass, vani, mani, effects and sounds are shown and counted, not edited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlaceList {
    #[default]
    Building,
    Shape,
    Tree,
    Dungeon,
}

/// A selected placement: list and index.
pub type PSel = (PlaceList, usize);

impl PlaceList {
    pub const ALL: [PlaceList; 4] = [PlaceList::Building, PlaceList::Shape, PlaceList::Tree, PlaceList::Dungeon];

    pub fn slot(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            PlaceList::Building => "building",
            PlaceList::Shape => "shape",
            PlaceList::Tree => "tree",
            PlaceList::Dungeon => "dungeon piece",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            PlaceList::Building => "buildings",
            PlaceList::Shape => "shapes",
            PlaceList::Tree => "trees",
            PlaceList::Dungeon => "dungeon pieces",
        }
    }

    /// The folder of the models in the client (`Entity/<dir>/<name>`; the pieces are `.dg` files of `world/dungeon`).
    pub fn folder(self) -> &'static str {
        match self {
            PlaceList::Building => "Entity/Building",
            PlaceList::Shape => "Entity/Shape",
            PlaceList::Tree => "Entity/Tree",
            PlaceList::Dungeon => "world/dungeon",
        }
    }

    pub fn list(self, m: &WldRaw) -> &RawPlaced {
        match self {
            PlaceList::Building => &m.building,
            PlaceList::Shape => &m.shape,
            PlaceList::Tree => &m.tree,
            PlaceList::Dungeon => &m.dungeon,
        }
    }

    pub fn list_mut(self, m: &mut WldRaw) -> &mut RawPlaced {
        match self {
            PlaceList::Building => &mut m.building,
            PlaceList::Shape => &mut m.shape,
            PlaceList::Tree => &mut m.tree,
            PlaceList::Dungeon => &mut m.dungeon,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- vectors

fn norm(v: V3) -> V3 {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 && l.is_finite() {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        v
    }
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The heading of a placement in degrees: `dir = (sin a, 0, cos a)` is `a` (the client's yaw, see `smod::placement_basis`).
pub fn yaw_deg(p: &Placement) -> f32 {
    p.dir[0].atan2(p.dir[2]).to_degrees()
}

fn turn_y(v: V3, delta: f32) -> V3 {
    let (s, c) = delta.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

/// The placement turned by `delta` radians about the world Y axis: `dir` and `up` turn together and are made unit length and
/// perpendicular again, so a long run of turns never shears a model. The position stays.
pub fn turned(p: &Placement, delta: f32) -> Placement {
    let dir = norm(turn_y(p.dir, delta));
    let up_t = turn_y(p.up, delta);
    // remove the part of `up` along `dir` (rounding), then make it unit length
    let d = dot(up_t, dir);
    let up = norm([up_t[0] - dir[0] * d, up_t[1] - dir[1] * d, up_t[2] - dir[2] * d]);
    Placement { index: p.index, pos: p.pos, dir, up }
}

/// The placement turned to the heading `deg`.
pub fn with_yaw(p: &Placement, deg: f32) -> Placement {
    turned(p, (deg - yaw_deg(p)).to_radians())
}

/// A new, upright placement of name `index` at `pos`, facing `yaw_deg`.
pub fn upright(index: i32, pos: V3, yaw: f32) -> Placement {
    let a = yaw.to_radians();
    Placement { index, pos, dir: [a.sin(), 0.0, a.cos()], up: [0.0, 1.0, 0.0] }
}

#[cfg(test)]
/// Whether `dir` and `up` are unit length and perpendicular (the rule every placement of the originals follows).
pub fn is_orthonormal(p: &Placement) -> bool {
    (dot(p.dir, p.dir) - 1.0).abs() < 1e-4 && (dot(p.up, p.up) - 1.0).abs() < 1e-4 && dot(p.dir, p.up).abs() < 1e-4
}

// ---------------------------------------------------------------------------------------------------------------- names

/// The index of `name` in the list (the same text, ignoring case: the client compares file names that way).
pub fn name_index(list: &RawPlaced, name: &str) -> Option<usize> {
    list.names.iter().position(|n| name_text(n).eq_ignore_ascii_case(name.trim()))
}

/// The text of a placement's model name, `None` for index -1 or one past the list (the client skips those).
pub fn model_name(list: &RawPlaced, p: &Placement) -> Option<String> {
    let n = usize::try_from(p.index).ok().and_then(|i| list.names.get(i))?;
    Some(name_text(n))
}

// ---------------------------------------------------------------------------------------------------------------- operations

/// The ops that add a placement of model `name` at `pos` facing `yaw` (degrees). The name is appended to the list only when the
/// list lacks it; the placement goes to the end of the list. Returns the ops and the index the new placement gets.
pub fn add_ops(m: &WldRaw, list: PlaceList, name: &str, pos: V3, yaw: f32, cp: Codepage) -> Result<(Vec<COp>, usize), String> {
    let l = list.list(m);
    let name = name.trim();
    if name.is_empty() {
        return Err("no model is chosen".to_string());
    }
    let mut ops = Vec::new();
    let index = match name_index(l, name) {
        Some(i) => i,
        None => {
            let mut buf: Name256 = [0; 256];
            cp.set_fixed(&mut buf, name)?;
            ops.push(COp::PlaceName(list, l.names.len(), CKind::Insert(buf)));
            l.names.len()
        }
    };
    let at = l.items.len();
    ops.push(COp::Place(list, at, CKind::Insert(upright(index as i32, pos, yaw))));
    Ok((ops, at))
}

/// The placement at `sel`, when the list has it.
pub fn get(m: &WldRaw, sel: PSel) -> Option<Placement> {
    sel.0.list(m).items.get(sel.1).copied()
}

/// One replace op per entry: `after` for each selected placement.
pub fn replace_ops(m: &WldRaw, edits: &[(PSel, Placement)]) -> Vec<COp> {
    edits.iter().filter_map(|(sel, after)| get(m, *sel).filter(|b| b != after).map(|before| COp::Place(sel.0, sel.1, CKind::Replace { before, after: *after }))).collect()
}

#[cfg(test)]
/// Moves every placement by `delta` (x, y, z).
pub fn move_ops(m: &WldRaw, sels: &[PSel], delta: V3) -> Vec<COp> {
    let edits: Vec<(PSel, Placement)> = sels
        .iter()
        .filter_map(|s| {
            let mut p = get(m, *s)?;
            p.pos = [p.pos[0] + delta[0], p.pos[1] + delta[1], p.pos[2] + delta[2]];
            Some((*s, p))
        })
        .collect();
    replace_ops(m, &edits)
}

/// Turns every placement by `deg` degrees about its own position.
pub fn rotate_ops(m: &WldRaw, sels: &[PSel], deg: f32) -> Vec<COp> {
    let edits: Vec<(PSel, Placement)> = sels.iter().filter_map(|s| Some((*s, turned(&get(m, *s)?, deg.to_radians())))).collect();
    replace_ops(m, &edits)
}

/// Sets the height of every placement to the ground under it.
pub fn snap_ops(m: &WldRaw, sels: &[PSel], ground: &dyn Fn(f32, f32) -> Option<f32>) -> Vec<COp> {
    let edits: Vec<(PSel, Placement)> = sels
        .iter()
        .filter_map(|s| {
            let mut p = get(m, *s)?;
            p.pos[1] = ground(p.pos[0], p.pos[2])?;
            Some((*s, p))
        })
        .collect();
    replace_ops(m, &edits)
}

/// Copies of the placements, offset by `offset`, appended to the end of their lists with the same names (no name changes).
/// Returns the ops and the new selections.
pub fn duplicate_ops(m: &WldRaw, sels: &[PSel], offset: V3) -> (Vec<COp>, Vec<PSel>) {
    let mut ops = Vec::new();
    let mut made = Vec::new();
    let mut next = [0usize; 4];
    for pl in PlaceList::ALL {
        next[pl.slot()] = pl.list(m).items.len();
    }
    let mut ordered: Vec<PSel> = sels.to_vec();
    ordered.sort();
    ordered.dedup();
    for s in ordered {
        let Some(mut p) = get(m, s) else { continue };
        p.pos = [p.pos[0] + offset[0], p.pos[1] + offset[1], p.pos[2] + offset[2]];
        let at = next[s.0.slot()];
        next[s.0.slot()] += 1;
        ops.push(COp::Place(s.0, at, CKind::Insert(p)));
        made.push((s.0, at));
    }
    (ops, made)
}

/// Removes the placements. Later indices of the same list shift down by the usual rule of a list; **the names stay**, so every
/// other placement keeps pointing at its model. Removed from the highest index down, so each op's index is valid when it runs.
pub fn delete_ops(m: &WldRaw, sels: &[PSel]) -> Vec<COp> {
    let mut ordered: Vec<PSel> = sels.to_vec();
    ordered.sort();
    ordered.dedup();
    ordered.reverse();
    ordered.into_iter().filter_map(|s| get(m, s).map(|p| COp::Place(s.0, s.1, CKind::Remove(p)))).collect()
}

/// Whether any op inserts or removes a placement (the selection indices of a tab do not survive that).
pub fn changes_structure(cops: &[COp]) -> bool {
    cops.iter().any(|c| matches!(c, COp::Place(_, _, CKind::Insert(_) | CKind::Remove(_)) | COp::Lists { .. }))
}

// ---------------------------------------------------------------------------------------------------------------- models

/// The bounds of a model in its own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: V3,
    pub max: V3,
}

/// What stands in for a model that has no file or cannot be read: a small box, so it can still be picked.
pub const STAND_IN: Bounds = Bounds { min: [-1.5, 0.0, -1.5], max: [1.5, 3.0, 1.5] };

/// Reads the bounds of a model from the client: the `.smod` box, or the `.dg` bounds of a dungeon piece.
pub fn load_bounds(src: &ClientSource, list: PlaceList, name: &str) -> Option<Bounds> {
    match list {
        PlaceList::Dungeon => src.dg(name).ok().map(|d| Bounds { min: d.bounds.min, max: d.bounds.max }),
        _ => {
            let dir = list.folder().rsplit('/').next().unwrap_or("Building");
            src.smod(dir, name).ok().map(|s| Bounds { min: s.min, max: s.max })
        }
    }
}

/// The bounds of the models seen so far, read from the client when first asked for (a model that cannot be read is remembered
/// as missing).
#[derive(Default)]
pub struct BoundsCache {
    map: HashMap<(PlaceList, String), Option<Bounds>>,
}

impl BoundsCache {
    pub fn forget(&mut self) {
        self.map.clear();
    }

    /// The bounds of `name`: the cached ones, else read from `src`. `None` = no file; callers use [`STAND_IN`].
    pub fn get(&mut self, src: Option<&ClientSource>, list: PlaceList, name: &str) -> Option<Bounds> {
        let key = (list, name.to_ascii_lowercase());
        if let Some(b) = self.map.get(&key) {
            return *b;
        }
        let b = src.and_then(|s| load_bounds(s, list, name));
        if src.is_some() {
            self.map.insert(key, b);
        }
        b
    }

    /// A value for tests.
    #[cfg(test)]
    pub fn put(&mut self, list: PlaceList, name: &str, b: Option<Bounds>) {
        self.map.insert((list, name.to_ascii_lowercase()), b);
    }
}

/// The eight corners of a placement's box in the world, in the order of `Draw::corners` of the 3D view: the lower face (min y)
/// counter-clockwise from the minimum corner, then the upper face.
pub fn world_corners(p: &Placement, b: &Bounds) -> [V3; 8] {
    let ax = placement_basis(p.dir, p.up);
    let at = |x: f32, y: f32, z: f32| [p.pos[0] + ax[0][0] * x + ax[1][0] * y + ax[2][0] * z, p.pos[1] + ax[0][1] * x + ax[1][1] * y + ax[2][1] * z, p.pos[2] + ax[0][2] * x + ax[1][2] * y + ax[2][2] * z];
    let (a, c) = (b.min, b.max);
    [at(a[0], a[1], a[2]), at(c[0], a[1], a[2]), at(c[0], a[1], c[2]), at(a[0], a[1], c[2]), at(a[0], c[1], a[2]), at(c[0], c[1], a[2]), at(c[0], c[1], c[2]), at(a[0], c[1], c[2])]
}

/// Where a ray meets the placed box: the distance along the ray (0 when it starts inside), `None` when it misses.
pub fn ray_box(origin: V3, dir: V3, p: &Placement, b: &Bounds) -> Option<f32> {
    let ax = placement_basis(p.dir, p.up);
    let rel = [origin[0] - p.pos[0], origin[1] - p.pos[1], origin[2] - p.pos[2]];
    // the basis is orthonormal: world to model space is the transpose
    let o = [dot(ax[0], rel), dot(ax[1], rel), dot(ax[2], rel)];
    let d = [dot(ax[0], dir), dot(ax[1], dir), dot(ax[2], dir)];
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for k in 0..3 {
        if d[k].abs() < 1e-9 {
            if o[k] < b.min[k] || o[k] > b.max[k] {
                return None;
            }
        } else {
            let (mut a, mut c) = ((b.min[k] - o[k]) / d[k], (b.max[k] - o[k]) / d[k]);
            if a > c {
                std::mem::swap(&mut a, &mut c);
            }
            t0 = t0.max(a);
            t1 = t1.min(c);
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// Distance of a point from the line through `origin` along `dir`.
pub fn line_distance(origin: V3, dir: V3, p: V3) -> f32 {
    let d = norm(dir);
    let r = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
    let c = [r[1] * d[2] - r[2] * d[1], r[2] * d[0] - r[0] * d[2], r[0] * d[1] - r[1] * d[0]];
    dot(c, c).sqrt()
}

/// A placement the ray hits, nearest first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub sel: PSel,
    pub t: f32,
}

/// Every placement of the shown lists whose box the ray meets, nearest first. `near` limits the work of a click: only placements whose
/// position is within that many units of the ray are tested (and only for those the model is read from the client).
pub fn pick(m: &WldRaw, cache: &mut BoundsCache, src: Option<&ClientSource>, lists: &[PlaceList], origin: V3, dir: V3, near: Option<f32>) -> Vec<Pick> {
    let mut hits = Vec::new();
    for &pl in lists {
        let l = pl.list(m);
        for (i, p) in l.items.iter().enumerate() {
            let Some(name) = model_name(l, p) else { continue };
            if let Some(r) = near {
                if line_distance(origin, dir, p.pos) > r {
                    continue;
                }
            }
            let b = cache.get(src, pl, &name).unwrap_or(STAND_IN);
            if let Some(t) = ray_box(origin, dir, p, &b) {
                hits.push(Pick { sel: (pl, i), t });
            }
        }
    }
    hits.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// The footprint of a placed box on the ground, four corners (x, z) in order.
pub fn footprint(p: &Placement, b: &Bounds) -> [[f32; 2]; 4] {
    let c = world_corners(p, b);
    [[c[0][0], c[0][2]], [c[1][0], c[1][2]], [c[2][0], c[2][2]], [c[3][0], c[3][2]]]
}

// ---------------------------------------------------------------------------------------------------------------- model lists

/// The model file names of one list, from the client's file list: `.smod` files of `Entity/<dir>`, `.dg` files of
/// `world/dungeon`. Sorted, one entry per name (case-insensitive).
pub fn model_names(files: &[String], list: PlaceList) -> Vec<String> {
    let prefix = format!("{}/", list.folder().to_ascii_lowercase());
    let ext = if list == PlaceList::Dungeon { ".dg" } else { ".smod" };
    let mut v: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let norm = f.replace('\\', "/");
            let lower = norm.to_ascii_lowercase();
            if !lower.starts_with(&prefix) || !lower.ends_with(ext) {
                return None;
            }
            let rest = &norm[prefix.len()..];
            (!rest.contains('/')).then(|| rest.to_string())
        })
        .collect();
    v.sort_by_key(|s| s.to_ascii_lowercase());
    v.dedup_by_key(|s| s.to_ascii_lowercase());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use crate::testutil::paired_doc;

    fn nm(s: &str) -> Name256 {
        let mut b = [0u8; 256];
        b[..s.len()].copy_from_slice(s.as_bytes());
        // garbage after the NUL, as in the originals
        for (i, x) in b[s.len() + 1..].iter_mut().enumerate().take(8) {
            *x = 0xA0 + i as u8;
        }
        b
    }

    /// A tab whose client half has three building names and four buildings, two trees of one name.
    fn placed_doc() -> Doc {
        let mut d = paired_doc();
        {
            let m = &mut d.part_mut().unwrap().wld.model;
            m.building = RawPlaced { names: vec![nm("a.smod"), nm("b.smod"), nm("c.smod")], items: vec![upright(0, [10.0, 1.0, 10.0], 0.0), upright(1, [20.0, 1.0, 10.0], 90.0), upright(2, [30.0, 1.0, 10.0], 180.0), upright(1, [40.0, 1.0, 10.0], 270.0)] };
            m.tree = RawPlaced { names: vec![nm("t.smod")], items: vec![upright(0, [5.0, 1.0, 5.0], 0.0), upright(0, [6.0, 1.0, 5.0], 10.0)] };
        }
        // what the model holds now counts as the saved state
        let p = d.part_mut().unwrap();
        p.base = p.lists();
        d
    }

    fn model(d: &Doc) -> &WldRaw {
        &d.part().unwrap().wld.model
    }

    fn apply(d: &mut Doc, label: &str, ops: Vec<COp>) {
        d.commit_client(label, ops).expect("a client edit");
    }

    fn names_of(m: &WldRaw, l: PlaceList) -> Vec<String> {
        l.list(m).names.iter().map(|n| name_text(n)).collect()
    }

    /// Adding a placement of a model the list already names reuses its index; a new model appends its name at the end; no
    /// existing placement changes, and undo and redo restore the list exactly (names included).
    #[test]
    fn placement_add_keeps_name_indices() {
        let mut d = placed_doc();
        let before = model(&d).building.clone();
        // known model: no new name
        let (ops, at) = add_ops(model(&d), PlaceList::Building, "B.SMOD", [50.0, 1.0, 50.0], 30.0, Codepage::Cp949).unwrap();
        assert_eq!(ops.len(), 1, "no name op for a name the list has");
        apply(&mut d, "add", ops);
        let m = model(&d);
        assert_eq!(m.building.names, before.names, "names unchanged");
        assert_eq!(m.building.items.len(), 5);
        assert_eq!((at, m.building.items[at].index), (4, 1), "the existing name's index");
        assert_eq!(&m.building.items[..4], &before.items[..], "no other placement changed");
        assert!(is_orthonormal(&m.building.items[at]) && (yaw_deg(&m.building.items[at]) - 30.0).abs() < 1e-3);
        // new model: the name is appended after the others, the old indices stay
        let (ops, at2) = add_ops(model(&d), PlaceList::Building, "d.smod", [60.0, 1.0, 50.0], 0.0, Codepage::Cp949).unwrap();
        assert_eq!(ops.len(), 2, "name, then placement");
        apply(&mut d, "add new model", ops);
        let m = model(&d);
        assert_eq!(names_of(m, PlaceList::Building), ["a.smod", "b.smod", "c.smod", "d.smod"]);
        assert_eq!(m.building.names[..3], before.names[..], "old names keep their bytes, garbage included");
        assert_eq!(m.building.items[at2].index, 3);
        assert_eq!(&m.building.items[..4], &before.items[..]);
        assert_eq!(d.undo.len(), 2);
        // undo takes the placement and the name back: the list is as it was
        d.undo();
        assert_eq!(names_of(model(&d), PlaceList::Building).len(), 3, "the appended name goes with the undo");
        d.undo();
        assert_eq!(model(&d).building, before);
        assert!(!d.client_dirty(), "back at the start is clean");
        d.redo();
        d.redo();
        assert_eq!(model(&d).building.names.len(), 4);
        assert_eq!(model(&d).building.items.len(), 6);
        // the model still writes back, and reads back the same
        let bytes = model(&d).to_bytes();
        let back = WldRaw::parse(&bytes, "x").expect("reads back");
        assert_eq!(back.building, model(&d).building);
        // a name that does not fit the 256-byte buffer is refused and changes nothing
        assert!(add_ops(model(&d), PlaceList::Tree, &"x".repeat(300), [0.0; 3], 0.0, Codepage::Cp949).is_err());
        assert!(add_ops(model(&d), PlaceList::Tree, "  ", [0.0; 3], 0.0, Codepage::Cp949).is_err());
    }

    /// Move, rotate and snap are one undo step each; `dir` and `up` stay unit length and perpendicular through a long run of turns,
    /// including from a tilted start; undo gives the exact bytes back.
    #[test]
    fn placement_move_rotate_undo() {
        let mut d = placed_doc();
        // a tilted placement (a tree leaning): dir and up still unit and perpendicular
        {
            let m = &mut d.part_mut().unwrap().wld.model;
            let tilt = 0.2f32;
            m.tree.items[1].up = [tilt.sin(), tilt.cos(), 0.0];
            m.tree.items[1].dir = [0.0, 0.0, 1.0];
            assert!(is_orthonormal(&m.tree.items[1]));
            let p = d.part_mut().unwrap();
            p.base = p.lists();
        }
        let l0 = d.part().unwrap().lists();
        let sels = [(PlaceList::Building, 1), (PlaceList::Tree, 1)];
        let ops = move_ops(model(&d), &sels, [3.0, 0.5, -2.0]);
        apply(&mut d, "move", ops);
        let m = model(&d);
        assert_eq!(m.building.items[1].pos, [23.0, 1.5, 8.0]);
        assert_eq!(m.tree.items[1].pos, [9.0, 1.5, 3.0]);
        assert_eq!(m.building.items[0], l0.places[0].items[0], "an unselected placement stays");
        assert_eq!(d.undo.len(), 1);
        for k in 0..400 {
            let deg = if k % 3 == 0 { 15.0 } else { -7.5 };
            let ops = rotate_ops(model(&d), &sels, deg);
            apply(&mut d, "turn", ops);
        }
        let m = model(&d);
        for p in [&m.building.items[1], &m.tree.items[1]] {
            assert!(is_orthonormal(p), "dir {:?} up {:?}", p.dir, p.up);
        }
        // 400 turns: +15 degrees on every third, -7.5 on the others
        let net = (0..400).map(|k| if k % 3 == 0 { 15.0f32 } else { -7.5 }).sum::<f32>();
        let want = (90.0 + net).rem_euclid(360.0);
        let got = yaw_deg(&m.building.items[1]).rem_euclid(360.0);
        assert!((got - want).abs() < 0.05 || (got - want).abs() > 359.95, "yaw {got} want {want}");
        // the tilt of the tree survives the turns (its up vector keeps the angle to the horizontal)
        assert!((m.tree.items[1].up[1] - 0.2f32.cos()).abs() < 1e-3, "tilt kept: {:?}", m.tree.items[1].up);
        // turn to an exact heading
        let p = with_yaw(&m.building.items[1], 45.0);
        assert!((yaw_deg(&p) - 45.0).abs() < 1e-3 && is_orthonormal(&p));
        // everything undoes one step at a time, back to the start's bytes
        while d.undo().is_some() {}
        assert_eq!(d.part().unwrap().lists(), l0);
        assert!(!d.client_dirty());
        // snap sets y from the ground function; no ground changes nothing
        let ops = snap_ops(model(&d), &sels, &|x, z| Some(x * 0.1 + z * 0.01));
        apply(&mut d, "snap", ops);
        assert_eq!(model(&d).building.items[1].pos[1], 20.0 * 0.1 + 10.0 * 0.01);
        let none: Vec<COp> = snap_ops(model(&d), &sels, &|_, _| None);
        assert!(none.is_empty());
    }

    /// Delete removes the placement only: every name stays (also the last user of a name), every other placement keeps its index
    /// into the names, and undo puts the placement back at its place.
    #[test]
    fn placement_delete_keeps_names() {
        let mut d = placed_doc();
        let l0 = d.part().unwrap().lists();
        // delete the only user of name 2, one user of name 1, and a tree
        let sels = [(PlaceList::Building, 2), (PlaceList::Building, 1), (PlaceList::Tree, 0)];
        let ops = delete_ops(model(&d), &sels);
        assert_eq!(ops.len(), 3);
        apply(&mut d, "delete", ops);
        let m = model(&d);
        assert_eq!(m.building.names, l0.places[0].names, "no name is removed");
        assert_eq!(m.tree.names, l0.places[2].names);
        assert_eq!(m.building.items.len(), 2);
        // what is left still names the same models
        let still: Vec<String> = m.building.items.iter().map(|p| model_name(&m.building, p).unwrap()).collect();
        assert_eq!(still, ["a.smod", "b.smod"], "the placements at 0 and 3 (b) keep their models");
        assert_eq!(m.tree.items.len(), 1);
        assert_eq!(model_name(&m.tree, &m.tree.items[0]).unwrap(), "t.smod");
        assert_eq!(d.undo.len(), 1, "one undo step");
        d.undo();
        assert_eq!(d.part().unwrap().lists(), l0);
        assert!(!d.client_dirty());
        d.redo();
        assert_eq!(model(&d).building.items.len(), 2);
        // a delete of nothing real does nothing
        assert!(delete_ops(model(&d), &[(PlaceList::Building, 99)]).is_empty());
        // duplicates of one selection delete once
        let ops = delete_ops(model(&d), &[(PlaceList::Building, 0), (PlaceList::Building, 0)]);
        assert_eq!(ops.len(), 1);
    }

    #[test]
    fn duplicate_appends_copies_with_the_same_names() {
        let mut d = placed_doc();
        let l0 = d.part().unwrap().lists();
        let (ops, made) = duplicate_ops(model(&d), &[(PlaceList::Building, 1), (PlaceList::Tree, 0), (PlaceList::Building, 3)], [2.0, 0.0, 2.0]);
        assert_eq!(made, [(PlaceList::Building, 4), (PlaceList::Building, 5), (PlaceList::Tree, 2)]);
        apply(&mut d, "duplicate", ops);
        let m = model(&d);
        assert_eq!(m.building.items[4].pos, [22.0, 1.0, 12.0]);
        assert_eq!(m.building.items[4].index, m.building.items[1].index);
        assert_eq!(m.building.names, l0.places[0].names);
        assert_eq!(m.tree.items.len(), 3);
        d.undo();
        assert_eq!(d.part().unwrap().lists(), l0);
    }

    /// The ray test follows the turned box, picks the nearest box first, and a missing model gets the stand-in box.
    #[test]
    fn picking_follows_the_turned_box() {
        let b = Bounds { min: [-5.0, 0.0, -1.0], max: [5.0, 4.0, 1.0] };
        let p = upright(0, [100.0, 0.0, 100.0], 0.0);
        // straight down at (104, 100): inside the 10 x 2 footprint (long side along x)
        assert!(ray_box([104.0, 50.0, 100.0], [0.0, -1.0, 0.0], &p, &b).is_some());
        assert!(ray_box([100.0, 50.0, 104.0], [0.0, -1.0, 0.0], &p, &b).is_none());
        // turned a quarter: the long side lies along z now
        let q = with_yaw(&p, 90.0);
        assert!(ray_box([100.0, 50.0, 104.0], [0.0, -1.0, 0.0], &q, &b).is_some() || ray_box([104.0, 50.0, 100.0], [0.0, -1.0, 0.0], &q, &b).is_some());
        assert!(ray_box([104.0, 50.0, 100.0], [0.0, -1.0, 0.0], &q, &b).is_none(), "the old long side is empty now");
        // the distance is the entry
        let t = ray_box([100.0, 50.0, 100.0], [0.0, -1.0, 0.0], &p, &b).unwrap();
        assert!((t - 46.0).abs() < 1e-3, "{t}");
        // a whole picking run over a model: nearest first
        let mut m = crate::testutil::tiny_wld(8);
        m.building = RawPlaced { names: vec![nm("a.smod")], items: vec![upright(0, [10.0, 0.0, 10.0], 0.0), upright(0, [10.0, 20.0, 10.0], 0.0), upright(-1, [10.0, 0.0, 10.0], 0.0)] };
        let mut cache = BoundsCache::default();
        cache.put(PlaceList::Building, "a.smod", Some(b));
        let hits = pick(&m, &mut cache, None, &PlaceList::ALL, [10.0, 100.0, 10.0], [0.0, -1.0, 0.0], None);
        assert_eq!(hits.iter().map(|h| h.sel).collect::<Vec<_>>(), [(PlaceList::Building, 1), (PlaceList::Building, 0)], "the higher one first; index -1 is skipped");
        let none = pick(&m, &mut cache, None, &[PlaceList::Tree], [10.0, 100.0, 10.0], [0.0, -1.0, 0.0], None);
        assert!(none.is_empty());
        // the near limit leaves out far placements
        let far = pick(&m, &mut cache, None, &PlaceList::ALL, [500.0, 100.0, 500.0], [0.0, -1.0, 0.0], Some(20.0));
        assert!(far.is_empty());
        // an unknown model is still pickable through the stand-in
        m.tree = RawPlaced { names: vec![nm("gone.smod")], items: vec![upright(0, [30.0, 0.0, 30.0], 0.0)] };
        let hits = pick(&m, &mut cache, None, &[PlaceList::Tree], [30.0, 100.0, 30.0], [0.0, -1.0, 0.0], None);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn model_lists_come_from_the_file_list() {
        let files: Vec<String> = ["Entity/Building/b2.smod", "entity/building/A1.smod", "Entity/Building/readme.txt", "Entity/Shape/s.smod", "Entity/Building/sub/x.smod", "world/dungeon/d1.dg", "World/Dungeon/d1_L0.dds", "entity/building/a1.SMOD"].map(String::from).to_vec();
        assert_eq!(model_names(&files, PlaceList::Building), ["A1.smod", "b2.smod"]);
        assert_eq!(model_names(&files, PlaceList::Shape), ["s.smod"]);
        assert_eq!(model_names(&files, PlaceList::Dungeon), ["d1.dg"]);
        assert!(model_names(&files, PlaceList::Tree).is_empty());
    }

    fn close3(a: V3, b: V3, eps: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
    }

    /// Review: every placement of the four lists in every `.wld` of the client goes through the heading functions
    /// unchanged: `with_yaw(p, yaw_deg(p))`, a turn there and back, and (for an upright one) `upright(yaw_deg(p))` give `dir` and
    /// `up` back; the documented basis `x = (cos a, 0, -sin a)` holds for a new placement. Counts what the originals hold.
    #[test]
    fn review_real_placements_round_trip_through_the_heading() {
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let Ok(src) = ClientSource::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: SHAIYA_CLIENT cannot be opened");
            return;
        };
        let (mut total, mut tilted, mut not_ortho, mut bad_index, mut maps) = (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut dungeon_items = 0usize;
        for path in src.wld_paths() {
            let Ok(bytes) = src.read(&path) else { continue };
            let Ok(m) = WldRaw::parse(&bytes, &path) else { continue };
            maps += 1;
            dungeon_items += m.dungeon.items.len();
            for pl in PlaceList::ALL {
                let l = pl.list(&m);
                for p in &l.items {
                    total += 1;
                    assert!(p.pos.iter().chain(&p.dir).chain(&p.up).all(|v| v.is_finite()), "{path} {pl:?}: {p:?}");
                    if p.index < -1 || p.index >= l.names.len() as i32 {
                        bad_index += 1;
                    }
                    if !is_orthonormal(p) {
                        not_ortho += 1;
                        continue;
                    }
                    let q = with_yaw(p, yaw_deg(p));
                    assert!(close3(q.dir, p.dir, 1e-4) && close3(q.up, p.up, 1e-4), "{path} {pl:?}: {p:?} -> {q:?}");
                    let r = turned(&turned(p, 0.7), -0.7);
                    assert!(close3(r.dir, p.dir, 1e-4) && close3(r.up, p.up, 1e-4), "{path} {pl:?}: {p:?} -> {r:?}");
                    if close3(p.up, [0.0, 1.0, 0.0], 1e-4) {
                        let u = upright(p.index, p.pos, yaw_deg(p));
                        assert!(close3(u.dir, p.dir, 1e-4), "{path} {pl:?}: {p:?} -> {u:?}");
                    } else {
                        tilted += 1;
                    }
                }
            }
        }
        for a in [0.0f32, 30.0, 90.0, 200.0] {
            let ax = placement_basis(upright(0, [0.0; 3], a).dir, [0.0, 1.0, 0.0]);
            let r = a.to_radians();
            assert!(close3(ax[0], [r.cos(), 0.0, -r.sin()], 1e-5) && close3(ax[2], [r.sin(), 0.0, r.cos()], 1e-5), "{a}: {ax:?}");
        }
        eprintln!("review: {maps} maps, {total} placements, {tilted} tilted, {not_ortho} not orthonormal, {bad_index} with an index outside the names, {dungeon_items} dungeon pieces");
        assert!(maps >= 80 && total > 10_000);
    }

    #[test]
    fn a_read_only_client_half_refuses_placement_edits() {
        let mut d = placed_doc();
        d.read_only = true;
        let ops = delete_ops(model(&d), &[(PlaceList::Building, 0)]);
        assert!(d.commit_client("delete", ops).is_err());
        assert_eq!(model(&d).building.items.len(), 4);
        d.read_only = false;
        d.set_client(crate::doc::ClientSlot::ReadOnly("test".to_string()));
        assert!(d.commit_client("delete", vec![]).is_err());
        let ops = vec![COp::Place(PlaceList::Building, 0, CKind::Remove(upright(0, [0.0; 3], 0.0)))];
        assert!(d.commit_client("delete", ops).is_err(), "a tab with a read-only .wld refuses too");
    }
}
