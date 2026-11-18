//! Record-level edits: a record of any list section, and the three operations (replace, insert, remove) that an undo step is
//! made of. Every operation can be inverted, so undo and redo are just "apply the inverse ops in reverse order".

use shaiya_svmap::{Aabb, Bind, MobArea, NpcEntry, Portal, RawA, Region, Run, SectionId, Svmap};

/// One record of a list section (section A, B, monster areas, NPCs, portals, binds, regions).
#[derive(Clone, Debug, PartialEq)]
pub enum Rec {
    A(RawA),
    B([f32; 3]),
    Mob(MobArea),
    Npc(NpcEntry),
    Portal(Portal),
    Bind(Bind),
    Region(Region),
}

impl Rec {
    pub fn section(&self) -> SectionId {
        match self {
            Rec::A(_) => SectionId::SectionA,
            Rec::B(_) => SectionId::SectionB,
            Rec::Mob(_) => SectionId::MobAreas,
            Rec::Npc(_) => SectionId::Npcs,
            Rec::Portal(_) => SectionId::Portals,
            Rec::Bind(_) => SectionId::Binds,
            Rec::Region(_) => SectionId::Regions,
        }
    }
}

/// Sections whose records are a list the user can select, in the viewport's picking order (topmost first).
pub const LIST_SECTIONS: [SectionId; 7] = [
    SectionId::Npcs,
    SectionId::Portals,
    SectionId::SectionB,
    SectionId::Binds,
    SectionId::MobAreas,
    SectionId::Regions,
    SectionId::SectionA,
];

pub fn count(m: &Svmap, s: SectionId) -> usize {
    match s {
        SectionId::SectionA => m.section_a.len(),
        SectionId::SectionB => m.points_b.len(),
        SectionId::MobAreas => m.mob_areas.len(),
        SectionId::Npcs => m.npcs.len(),
        SectionId::Portals => m.portals.len(),
        SectionId::Binds => m.binds.len(),
        SectionId::Regions => m.regions.len(),
        SectionId::Size | SectionId::Blocked => 0,
    }
}

pub fn get(m: &Svmap, s: SectionId, i: usize) -> Option<Rec> {
    match s {
        SectionId::SectionA => m.section_a.get(i).copied().map(Rec::A),
        SectionId::SectionB => m.points_b.get(i).copied().map(Rec::B),
        SectionId::MobAreas => m.mob_areas.get(i).cloned().map(Rec::Mob),
        SectionId::Npcs => m.npcs.get(i).cloned().map(Rec::Npc),
        SectionId::Portals => m.portals.get(i).copied().map(Rec::Portal),
        SectionId::Binds => m.binds.get(i).cloned().map(Rec::Bind),
        SectionId::Regions => m.regions.get(i).copied().map(Rec::Region),
        SectionId::Size | SectionId::Blocked => None,
    }
}

/// Replaces record `i`; false when the index or the record's section does not fit.
pub fn set(m: &mut Svmap, i: usize, rec: Rec) -> bool {
    match rec {
        Rec::A(r) => m.section_a.get_mut(i).map(|x| *x = r).is_some(),
        Rec::B(r) => m.points_b.get_mut(i).map(|x| *x = r).is_some(),
        Rec::Mob(r) => m.mob_areas.get_mut(i).map(|x| *x = r).is_some(),
        Rec::Npc(r) => m.npcs.get_mut(i).map(|x| *x = r).is_some(),
        Rec::Portal(r) => m.portals.get_mut(i).map(|x| *x = r).is_some(),
        Rec::Bind(r) => m.binds.get_mut(i).map(|x| *x = r).is_some(),
        Rec::Region(r) => m.regions.get_mut(i).map(|x| *x = r).is_some(),
    }
}

/// Inserts at `i` (clamped to the end).
pub fn insert(m: &mut Svmap, i: usize, rec: Rec) {
    match rec {
        Rec::A(r) => m.section_a.insert(i.min(m.section_a.len()), r),
        Rec::B(r) => m.points_b.insert(i.min(m.points_b.len()), r),
        Rec::Mob(r) => m.mob_areas.insert(i.min(m.mob_areas.len()), r),
        Rec::Npc(r) => m.npcs.insert(i.min(m.npcs.len()), r),
        Rec::Portal(r) => m.portals.insert(i.min(m.portals.len()), r),
        Rec::Bind(r) => m.binds.insert(i.min(m.binds.len()), r),
        Rec::Region(r) => m.regions.insert(i.min(m.regions.len()), r),
    }
}

pub fn remove(m: &mut Svmap, s: SectionId, i: usize) -> Option<Rec> {
    if i >= count(m, s) {
        return None;
    }
    Some(match s {
        SectionId::SectionA => Rec::A(m.section_a.remove(i)),
        SectionId::SectionB => Rec::B(m.points_b.remove(i)),
        SectionId::MobAreas => Rec::Mob(m.mob_areas.remove(i)),
        SectionId::Npcs => Rec::Npc(m.npcs.remove(i)),
        SectionId::Portals => Rec::Portal(m.portals.remove(i)),
        SectionId::Binds => Rec::Bind(m.binds.remove(i)),
        SectionId::Regions => Rec::Region(m.regions.remove(i)),
        SectionId::Size | SectionId::Blocked => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpKind {
    Replace { before: Rec, after: Rec },
    Insert(Rec),
    Remove(Rec),
    /// Inverts these blocked cells (the same op undoes itself); `index` is unused.
    Cells(Vec<Run>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub index: usize,
    pub kind: OpKind,
}

impl Op {
    pub fn replace(index: usize, before: Rec, after: Rec) -> Op {
        Op { index, kind: OpKind::Replace { before, after } }
    }

    pub fn insert(index: usize, rec: Rec) -> Op {
        Op { index, kind: OpKind::Insert(rec) }
    }

    pub fn remove(index: usize, rec: Rec) -> Op {
        Op { index, kind: OpKind::Remove(rec) }
    }

    /// Inverts the blocked state of the cells in `runs`.
    pub fn cells(runs: Vec<Run>) -> Op {
        Op { index: 0, kind: OpKind::Cells(runs) }
    }

    pub fn section(&self) -> SectionId {
        match &self.kind {
            OpKind::Replace { after, .. } => after.section(),
            OpKind::Insert(r) | OpKind::Remove(r) => r.section(),
            OpKind::Cells(_) => SectionId::Blocked,
        }
    }

    pub fn apply(&self, m: &mut Svmap) {
        match &self.kind {
            OpKind::Replace { after, .. } => {
                set(m, self.index, after.clone());
            }
            OpKind::Insert(r) => insert(m, self.index, r.clone()),
            OpKind::Remove(r) => {
                remove(m, r.section(), self.index);
            }
            OpKind::Cells(runs) => m.blocked.flip_runs(runs),
        }
    }

    pub fn invert(&self) -> Op {
        let kind = match &self.kind {
            OpKind::Replace { before, after } => OpKind::Replace { before: after.clone(), after: before.clone() },
            OpKind::Insert(r) => OpKind::Remove(r.clone()),
            OpKind::Remove(r) => OpKind::Insert(r.clone()),
            OpKind::Cells(runs) => OpKind::Cells(runs.clone()),
        };
        Op { index: self.index, kind }
    }
}

/// Bounding box (x, z) of a record, with a minimum size for points, in world units: (min x, min z, max x, max z).
pub fn bounds_xz(r: &Rec) -> (f32, f32, f32, f32) {
    fn bx(min: [f32; 3], max: [f32; 3]) -> (f32, f32, f32, f32) {
        (min[0].min(max[0]), min[2].min(max[2]), min[0].max(max[0]), min[2].max(max[2]))
    }
    match r {
        Rec::A(a) => (a.pos[0], a.pos[2], a.pos[0], a.pos[2]),
        Rec::B(p) => (p[0], p[2], p[0], p[2]),
        Rec::Mob(a) => bx(a.min, a.max),
        Rec::Npc(n) => {
            let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in &n.points {
                b = (b.0.min(p.pos[0]), b.1.min(p.pos[2]), b.2.max(p.pos[0]), b.3.max(p.pos[2]));
            }
            if n.points.is_empty() {
                (0.0, 0.0, 0.0, 0.0)
            } else {
                b
            }
        }
        Rec::Portal(p) => (p.pos[0], p.pos[2], p.pos[0], p.pos[2]),
        Rec::Bind(b) => {
            let mut o = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for x in &b.boxes {
                let t = bx(x.min, x.max);
                o = (o.0.min(t.0), o.1.min(t.1), o.2.max(t.2), o.3.max(t.3));
            }
            if b.boxes.is_empty() {
                (0.0, 0.0, 0.0, 0.0)
            } else {
                o
            }
        }
        Rec::Region(r) => bx(r.min, r.max),
    }
}

/// Moves a record by (dx, dz); y is kept.
pub fn translate(r: &mut Rec, dx: f32, dz: f32) {
    fn mv(p: &mut [f32; 3], dx: f32, dz: f32) {
        p[0] += dx;
        p[2] += dz;
    }
    match r {
        Rec::A(a) => mv(&mut a.pos, dx, dz),
        Rec::B(p) => mv(p, dx, dz),
        Rec::Mob(a) => {
            mv(&mut a.min, dx, dz);
            mv(&mut a.max, dx, dz);
        }
        Rec::Npc(n) => {
            for p in &mut n.points {
                mv(&mut p.pos, dx, dz);
            }
        }
        Rec::Portal(p) => mv(&mut p.pos, dx, dz),
        Rec::Bind(b) => {
            for x in &mut b.boxes {
                mv(&mut x.min, dx, dz);
                mv(&mut x.max, dx, dz);
            }
        }
        Rec::Region(g) => {
            mv(&mut g.min, dx, dz);
            mv(&mut g.max, dx, dz);
        }
    }
}

/// Moves a record up or down by `dy` (x and z are kept).
pub fn translate_y(r: &mut Rec, dy: f32) {
    match r {
        Rec::A(a) => a.pos[1] += dy,
        Rec::B(p) => p[1] += dy,
        Rec::Mob(a) => {
            a.min[1] += dy;
            a.max[1] += dy;
        }
        Rec::Npc(n) => {
            for p in &mut n.points {
                p.pos[1] += dy;
            }
        }
        Rec::Portal(p) => p.pos[1] += dy,
        Rec::Bind(b) => {
            for x in &mut b.boxes {
                x.min[1] += dy;
                x.max[1] += dy;
            }
        }
        Rec::Region(g) => {
            g.min[1] += dy;
            g.max[1] += dy;
        }
    }
}

/// Puts every point of a record on the ground, and boxes on the lowest ground under them (y1 = that, the height kept; monster
/// areas get the originals' 2-unit slab). `ground` may answer NaN where there is no ground (a dungeon cell without a floor):
/// such a point keeps its y, and a box ignores those samples.
pub fn drop_to_ground(r: &mut Rec, ground: &dyn Fn(f32, f32) -> f32) {
    fn set(p: &mut [f32; 3], ground: &dyn Fn(f32, f32) -> f32) {
        let g = ground(p[0], p[2]);
        if g.is_finite() {
            p[1] = g;
        }
    }
    match r {
        Rec::A(a) => set(&mut a.pos, ground),
        Rec::B(p) => set(p, ground),
        Rec::Mob(a) => {
            let y = lowest_ground(a.min, a.max, ground);
            if y.is_finite() {
                a.min[1] = y;
                a.max[1] = y + 2.0;
            }
        }
        Rec::Npc(n) => {
            for p in &mut n.points {
                set(&mut p.pos, ground);
            }
        }
        Rec::Portal(p) => set(&mut p.pos, ground),
        Rec::Bind(b) => {
            for x in &mut b.boxes {
                let y = lowest_ground(x.min, x.max, ground);
                if y.is_finite() {
                    let h = (x.max[1] - x.min[1]).max(2.0);
                    x.min[1] = y;
                    x.max[1] = y + h;
                }
            }
        }
        Rec::Region(g) => {
            let y = lowest_ground(g.min, g.max, ground);
            if y.is_finite() {
                let h = (g.max[1] - g.min[1]).max(2.0);
                g.min[1] = y;
                g.max[1] = y + h;
            }
        }
    }
}

fn lowest_ground(min: [f32; 3], max: [f32; 3], ground: &dyn Fn(f32, f32) -> f32) -> f32 {
    let (x0, x1) = (min[0].min(max[0]), min[0].max(max[0]));
    let (z0, z1) = (min[2].min(max[2]), min[2].max(max[2]));
    let mut lo = f32::MAX;
    let n = 8;
    for i in 0..=n {
        for j in 0..=n {
            let x = x0 + (x1 - x0) * i as f32 / n as f32;
            let z = z0 + (z1 - z0) * j as f32 / n as f32;
            lo = lo.min(ground(x, z)); // f32::min ignores NaN
        }
    }
    if lo == f32::MAX {
        f32::NAN
    } else {
        lo
    }
}

/// Every record of one list section, in order.
pub fn records(m: &Svmap, s: SectionId) -> Vec<Rec> {
    (0..count(m, s)).filter_map(|i| get(m, s, i)).collect()
}

/// Whether two records are equal bit for bit (`-0.0` is not `0.0`, NaN equals itself when the bits agree): the comparison the
/// diff and the recovery ops use, through the `Debug` text that prints the shortest decimal of every float.
pub fn same_bits(a: &Rec, b: &Rec) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// Ops that turn the list `from` into the list `to` inside one section: equal records at both ends are skipped, the records in
/// between are replaced pairwise, the surplus is removed or inserted. Applied in order, so the removals all use the same index.
pub fn list_ops(from: &[Rec], to: &[Rec]) -> Vec<Op> {
    let mut start = 0;
    while start < from.len() && start < to.len() && same_bits(&from[start], &to[start]) {
        start += 1;
    }
    let (mut ea, mut eb) = (from.len(), to.len());
    while ea > start && eb > start && same_bits(&from[ea - 1], &to[eb - 1]) {
        ea -= 1;
        eb -= 1;
    }
    let (a, b) = (&from[start..ea], &to[start..eb]);
    let common = a.len().min(b.len());
    let mut ops = Vec::new();
    for k in 0..common {
        if !same_bits(&a[k], &b[k]) {
            ops.push(Op::replace(start + k, a[k].clone(), b[k].clone()));
        }
    }
    for r in &a[common..] {
        ops.push(Op::remove(start + common, r.clone()));
    }
    for (k, r) in b.iter().enumerate().skip(common) {
        ops.push(Op::insert(start + k, r.clone()));
    }
    ops
}

/// Ops that turn `from` into `to` (per section: [`list_ops`]; the blocked cells as one cell-diff op). `None` when the two have
/// another size: the size cannot be edited.
pub fn diff_ops(from: &Svmap, to: &Svmap) -> Option<Vec<Op>> {
    if from.size != to.size {
        return None;
    }
    let runs = from.blocked.diff_runs(&to.blocked)?;
    let mut ops = Vec::new();
    if !runs.is_empty() {
        ops.push(Op::cells(runs));
    }
    for s in [SectionId::SectionA, SectionId::SectionB, SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions] {
        ops.extend(list_ops(&records(from, s), &records(to, s)));
    }
    Some(ops)
}

pub fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
    Aabb { min, max }
}

// ---------------------------------------------------------------------------------------------------------------- align, distribute

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    /// Larger z.
    North,
    /// Smaller z.
    South,
    CentreX,
    CentreZ,
}

impl Align {
    pub const ALL: [Align; 6] = [Align::Left, Align::Right, Align::North, Align::South, Align::CentreX, Align::CentreZ];

    pub fn label(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Right => "right",
            Align::North => "north",
            Align::South => "south",
            Align::CentreX => "centre x",
            Align::CentreZ => "centre z",
        }
    }
}

fn union_bounds(recs: &[Rec]) -> (f32, f32, f32, f32) {
    let mut u = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for r in recs {
        let b = bounds_xz(r);
        u = (u.0.min(b.0), u.1.min(b.1), u.2.max(b.2), u.3.max(b.3));
    }
    u
}

/// The records moved so their edges (or centres) line up: left = the smallest min x of the group, right = the largest max x,
/// north = the largest max z, south = the smallest min z, centre = the middle of the group's bounds. Same order as `recs`.
pub fn align(recs: &[Rec], how: Align) -> Vec<Rec> {
    let u = union_bounds(recs);
    recs.iter()
        .map(|r| {
            let b = bounds_xz(r);
            let (dx, dz) = match how {
                Align::Left => (u.0 - b.0, 0.0),
                Align::Right => (u.2 - b.2, 0.0),
                Align::North => (0.0, u.3 - b.3),
                Align::South => (0.0, u.1 - b.1),
                Align::CentreX => ((u.0 + u.2) * 0.5 - (b.0 + b.2) * 0.5, 0.0),
                Align::CentreZ => (0.0, (u.1 + u.3) * 0.5 - (b.1 + b.3) * 0.5),
            };
            let mut r = r.clone();
            translate(&mut r, dx, dz);
            r
        })
        .collect()
}

/// Equal gaps between the centres along x (`on_x`) or z: the two outermost records stay, the others are spread evenly between
/// them in the order of their centres. Fewer than three records come back unchanged. Same order as `recs`.
pub fn distribute(recs: &[Rec], on_x: bool) -> Vec<Rec> {
    let centre = |r: &Rec| {
        let b = bounds_xz(r);
        if on_x {
            (b.0 + b.2) * 0.5
        } else {
            (b.1 + b.3) * 0.5
        }
    };
    let n = recs.len();
    let mut out = recs.to_vec();
    if n < 3 {
        return out;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| centre(&recs[*a]).total_cmp(&centre(&recs[*b])));
    let (first, last) = (centre(&recs[order[0]]), centre(&recs[order[n - 1]]));
    for (k, i) in order.iter().enumerate() {
        let want = first + (last - first) * k as f32 / (n - 1) as f32;
        let d = want - centre(&recs[*i]);
        if on_x {
            translate(&mut out[*i], d, 0.0);
        } else {
            translate(&mut out[*i], 0.0, d);
        }
    }
    out
}

/// The point a record is snapped by: the lower corner of a box (the first box of a bind area), the first point of an NPC, else
/// the position.
pub fn snap_anchor(r: &Rec) -> [f32; 2] {
    match r {
        Rec::Npc(n) => n.points.first().map_or([0.0, 0.0], |p| [p.pos[0], p.pos[2]]),
        Rec::Bind(b) => b.boxes.first().map_or([0.0, 0.0], |x| [x.min[0].min(x.max[0]), x.min[2].min(x.max[2])]),
        other => {
            let b = bounds_xz(other);
            [b.0, b.1]
        }
    }
}

/// Snap a value to a step (computed in f64 so 12.3 stays the f32 nearest to 12.3).
pub fn snap_to(v: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return v;
    }
    // the step as its shortest decimal (0.1f32 is 0.10000000149 as an f64)
    let st: f64 = format!("{step}").parse().unwrap_or(step as f64);
    ((v as f64 / st).round() * st) as f32
}

/// The point a record is listed and compared by: its position, or the lower corner of its first box.
pub fn anchor_of(r: &Rec) -> [f32; 2] {
    match r {
        Rec::A(a) => [a.pos[0], a.pos[2]],
        Rec::B(p) => [p[0], p[2]],
        Rec::Mob(a) => [a.min[0].min(a.max[0]), a.min[2].min(a.max[2])],
        Rec::Npc(n) => n.points.first().map_or([0.0, 0.0], |p| [p.pos[0], p.pos[2]]),
        Rec::Portal(p) => [p.pos[0], p.pos[2]],
        Rec::Bind(b) => b.boxes.first().map_or([0.0, 0.0], |x| [x.min[0].min(x.max[0]), x.min[2].min(x.max[2])]),
        Rec::Region(r) => [r.min[0].min(r.max[0]), r.min[2].min(r.max[2])],
    }
}

/// Moves each record so its [`snap_anchor`] lands on the grid (`step` in world units; the anchor itself is set exactly).
pub fn snap_to_grid(recs: &[Rec], step: f32) -> Vec<Rec> {
    recs.iter()
        .map(|r| {
            let a = snap_anchor(r);
            let (sx, sz) = (snap_to(a[0], step), snap_to(a[1], step));
            let mut r = r.clone();
            translate(&mut r, sx - a[0], sz - a[1]);
            set_snap_anchor(&mut r, sx, sz);
            r
        })
        .collect()
}

fn set_snap_anchor(r: &mut Rec, x: f32, z: f32) {
    fn corner(min: &mut [f32; 3], max: &mut [f32; 3], x: f32, z: f32) {
        if min[0] <= max[0] {
            min[0] = x;
        } else {
            max[0] = x;
        }
        if min[2] <= max[2] {
            min[2] = z;
        } else {
            max[2] = z;
        }
    }
    match r {
        Rec::A(a) => {
            a.pos[0] = x;
            a.pos[2] = z;
        }
        Rec::B(p) => {
            p[0] = x;
            p[2] = z;
        }
        Rec::Portal(p) => {
            p.pos[0] = x;
            p.pos[2] = z;
        }
        Rec::Npc(n) => {
            if let Some(p) = n.points.first_mut() {
                p.pos[0] = x;
                p.pos[2] = z;
            }
        }
        Rec::Mob(a) => corner(&mut a.min, &mut a.max, x, z),
        Rec::Region(g) => corner(&mut g.min, &mut g.max, x, z),
        Rec::Bind(b) => {
            if let Some(f) = b.boxes.first_mut() {
                corner(&mut f.min, &mut f.max, x, z);
            }
        }
    }
}

/// Every box of monster areas and regions takes the first record's width and depth around its own centre (y is kept). Records
/// that are not such a box, and the first one, are unchanged. `None` when the first record is not a box.
pub fn same_size(recs: &[Rec]) -> Option<Vec<Rec>> {
    let first = recs.first()?;
    let (min, max) = match first {
        Rec::Mob(a) => (a.min, a.max),
        Rec::Region(g) => (g.min, g.max),
        _ => return None,
    };
    let (w, d) = ((max[0] - min[0]).abs(), (max[2] - min[2]).abs());
    Some(
        recs.iter()
            .enumerate()
            .map(|(i, r)| {
                let mut r = r.clone();
                if i > 0 {
                    let fix = |mn: &mut [f32; 3], mx: &mut [f32; 3]| {
                        let (cx, cz) = ((mn[0] + mx[0]) * 0.5, (mn[2] + mx[2]) * 0.5);
                        mn[0] = cx - w * 0.5;
                        mx[0] = cx + w * 0.5;
                        mn[2] = cz - d * 0.5;
                        mx[2] = cz + d * 0.5;
                    };
                    match &mut r {
                        Rec::Mob(a) => fix(&mut a.min, &mut a.max),
                        Rec::Region(g) => fix(&mut g.min, &mut g.max),
                        _ => {}
                    }
                }
                r
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------------------------------- batch edits

/// The one value all of `vals` share, or `None` when they differ (or there are none): what a batch field shows ("mixed").
pub fn common<T: PartialEq + Copy>(vals: impl IntoIterator<Item = T>) -> Option<T> {
    let mut it = vals.into_iter();
    let first = it.next()?;
    it.all(|v| v == first).then_some(first)
}

/// Replaces monster `from` with `to` in every entry of the area; false when it has none.
pub fn mob_replace(a: &mut MobArea, from: u32, to: u32) -> bool {
    let mut hit = false;
    for e in a.mobs.iter_mut().filter(|e| e.mob == from) {
        e.mob = to;
        hit = true;
    }
    hit
}

/// Sets the count of monster `mob` (every entry when `None`).
pub fn mob_set_count(a: &mut MobArea, mob: Option<u32>, count: u32) -> bool {
    let mut hit = false;
    for e in a.mobs.iter_mut().filter(|e| mob.is_none_or(|m| e.mob == m)) {
        e.count = count;
        hit = true;
    }
    hit
}

/// Removes every entry of monster `mob`.
pub fn mob_remove(a: &mut MobArea, mob: u32) -> bool {
    let n = a.mobs.len();
    a.mobs.retain(|e| e.mob != mob);
    a.mobs.len() != n
}

/// Makes the slab `h` high: y2 = y1 + h.
pub fn mob_set_height(a: &mut MobArea, h: f32) {
    // a negative or non-finite height would turn the slab upside down
    let h = if h.is_finite() { h.max(0.0) } else { 2.0 };
    a.max[1] = a.min[1].min(a.max[1]) + h;
    a.min[1] = a.min[1].min(a.max[1]);
}

/// Moves each side outwards by `d` (inwards when negative), x and z; a box cannot shrink past a 1-unit square at its centre.
pub fn mob_grow(a: &mut MobArea, d: f32) {
    for ax in [0usize, 2] {
        let (lo, hi) = (a.min[ax].min(a.max[ax]), a.min[ax].max(a.max[ax]));
        let c = (lo + hi) * 0.5;
        let half = ((hi - lo) * 0.5 + d).max(0.5);
        a.min[ax] = c - half;
        a.max[ax] = c + half;
    }
}

// ---------------------------------------------------------------------------------------------------------------- clipboard JSON

pub fn section_key(s: SectionId) -> &'static str {
    match s {
        SectionId::SectionA => "a",
        SectionId::SectionB => "b",
        SectionId::MobAreas => "mob_areas",
        SectionId::Npcs => "npcs",
        SectionId::Portals => "portals",
        SectionId::Binds => "binds",
        SectionId::Regions => "regions",
        SectionId::Size | SectionId::Blocked => "",
    }
}

/// The text copy puts on the OS clipboard: `{"svmap-editor":1,"map":<id>,"records":[{"section":"mob_areas","record":{...}}]}`.
/// Floats are the shortest text that reads back to the same f32. `None` when a record holds a NaN or infinity (JSON cannot carry it).
pub fn clipboard_json(map_id: Option<u32>, recs: &[Rec]) -> Option<String> {
    let mut out = format!("{{\"svmap-editor\":1,\"map\":{},\"records\":[", map_id.map_or("null".to_string(), |m| m.to_string()));
    for (i, r) in recs.iter().enumerate() {
        let body = match r {
            Rec::A(x) => serde_json::to_string(x),
            Rec::B(x) => serde_json::to_string(x),
            Rec::Mob(x) => serde_json::to_string(x),
            Rec::Npc(x) => serde_json::to_string(x),
            Rec::Portal(x) => serde_json::to_string(x),
            Rec::Bind(x) => serde_json::to_string(x),
            Rec::Region(x) => serde_json::to_string(x),
        }
        .ok()?;
        // serde_json writes a NaN or infinity as null, which would not read back
        if body.contains("null") {
            return None;
        }
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("{{\"section\":\"{}\",\"record\":{body}}}", section_key(r.section())));
    }
    out.push_str("]}");
    Some(out)
}

/// The records of text made by [`clipboard_json`]; `None` for any other text.
pub fn parse_clipboard(text: &str) -> Option<Vec<Rec>> {
    let t = text.trim_start();
    if !t.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(t).ok()?;
    v.get("svmap-editor")?.as_u64()?;
    let mut out = Vec::new();
    for item in v.get("records")?.as_array()? {
        let rec = item.get("record")?.clone();
        out.push(match item.get("section")?.as_str()? {
            "a" => Rec::A(serde_json::from_value(rec).ok()?),
            "b" => Rec::B(serde_json::from_value(rec).ok()?),
            "mob_areas" => Rec::Mob(serde_json::from_value(rec).ok()?),
            "npcs" => Rec::Npc(serde_json::from_value(rec).ok()?),
            "portals" => Rec::Portal(serde_json::from_value(rec).ok()?),
            "binds" => Rec::Bind(serde_json::from_value(rec).ok()?),
            "regions" => Rec::Region(serde_json::from_value(rec).ok()?),
            _ => return None,
        });
    }
    // a number too large for an f32 reads as infinity: such records would spread NaN through the paste offset, so the text is
    // refused (the same rule as `clipboard_json`, which writes non-finite numbers as null)
    if out.is_empty() || clipboard_json(None, &out).is_none() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_keeps_the_nearest_f32() {
        assert_eq!(snap_to(12.34, 0.1), 12.3);
        assert_eq!(snap_to(12.36, 0.1), 12.4);
        assert_eq!(snap_to(13.0, 4.0), 12.0);
        assert_eq!(snap_to(5.0, 0.0), 5.0);
    }
    use shaiya_svmap::{MobEntry, NpcPoint};

    fn map() -> Svmap {
        let mut m = Svmap::new(64);
        m.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [5.0, 2.0, 5.0], mobs: vec![MobEntry { mob: 1, count: 2 }] });
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [3.0, 0.0, 3.0], dir: 0.0 }] });
        m
    }

    #[test]
    fn ops_invert() {
        let mut m = map();
        let orig = m.clone();
        let before = get(&m, SectionId::MobAreas, 0).unwrap();
        let mut after = before.clone();
        translate(&mut after, 2.0, 3.0);
        let ops = vec![
            Op::replace(0, before, after),
            Op::insert(0, Rec::B([1.0, 2.0, 3.0])),
            Op::remove(0, get(&m, SectionId::Npcs, 0).unwrap()),
        ];
        for o in &ops {
            o.apply(&mut m);
        }
        assert_ne!(m, orig);
        assert_eq!(m.mob_areas[0].min[0], 3.0);
        assert_eq!(m.points_b.len(), 1);
        assert!(m.npcs.is_empty());
        for o in ops.iter().rev() {
            o.invert().apply(&mut m);
        }
        assert_eq!(m, orig);
    }

    fn area(x0: f32, z0: f32, x1: f32, z1: f32) -> Rec {
        Rec::Mob(MobArea { min: [x0, 0.0, z0], max: [x1, 2.0, z1], mobs: vec![MobEntry { mob: 1, count: 1 }] })
    }

    fn pt(x: f32, z: f32) -> Rec {
        Rec::Portal(Portal { pos: [x, 0.0, z], country: 0, level_min: 0, level_max: 0, to_map: 0, to_pos: [0.0; 3] })
    }

    #[test]
    fn align_distribute_snap() {
        let boxes = vec![area(10.0, 5.0, 20.0, 15.0), area(30.0, 0.0, 34.0, 8.0), area(50.0, 20.0, 60.0, 22.0)];
        let b = |r: &Rec| bounds_xz(r);
        // left / right / north / south
        let l = align(&boxes, Align::Left);
        assert!(l.iter().all(|r| b(r).0 == 10.0));
        assert_eq!(b(&l[1]).2, 14.0, "width kept");
        let r = align(&boxes, Align::Right);
        assert!(r.iter().all(|x| b(x).2 == 60.0));
        let n = align(&boxes, Align::North);
        assert!(n.iter().all(|x| b(x).3 == 22.0));
        let s = align(&boxes, Align::South);
        assert!(s.iter().all(|x| b(x).1 == 0.0));
        // centres: the middle of the group's bounds (x 10..60 -> 35, z 0..22 -> 11)
        let cx = align(&boxes, Align::CentreX);
        assert!(cx.iter().all(|x| (b(x).0 + b(x).2) * 0.5 == 35.0));
        let cz = align(&boxes, Align::CentreZ);
        assert!(cz.iter().all(|x| (b(x).1 + b(x).3) * 0.5 == 11.0));
        // y is never touched
        assert!(matches!(&l[0], Rec::Mob(a) if a.min[1] == 0.0 && a.max[1] == 2.0));

        // distribute: centres 15, 32, 55 -> the middle one moves to 35 (ends fixed), in any input order
        let d = distribute(&boxes, true);
        assert_eq!((b(&d[0]).0 + b(&d[0]).2) * 0.5, 15.0);
        assert_eq!((b(&d[2]).0 + b(&d[2]).2) * 0.5, 55.0);
        assert_eq!((b(&d[1]).0 + b(&d[1]).2) * 0.5, 35.0);
        let shuffled = vec![boxes[2].clone(), boxes[0].clone(), boxes[1].clone()];
        let d2 = distribute(&shuffled, true);
        assert_eq!((b(&d2[2]).0 + b(&d2[2]).2) * 0.5, 35.0, "the box that was in the middle");
        let dz = distribute(&boxes, false);
        // z centres 10, 4, 21: in order 4, 10, 21, so the box at 10 moves to 12.5 and the ends stay
        assert_eq!(((b(&dz[0]).1 + b(&dz[0]).3) * 0.5, (b(&dz[1]).1 + b(&dz[1]).3) * 0.5, (b(&dz[2]).1 + b(&dz[2]).3) * 0.5), (12.5, 4.0, 21.0));
        assert_eq!(distribute(&boxes[..2], true), boxes[..2].to_vec(), "fewer than three: unchanged");

        // snap: a box by its lower corner, a point by itself
        #[allow(clippy::approx_constant)]
        let sn = snap_to_grid(&[area(10.3, 5.6, 20.3, 15.6), pt(3.14, 2.71)], 0.5);
        assert!(matches!(&sn[0], Rec::Mob(a) if a.min[0] == 10.5 && a.min[2] == 5.5 && (a.max[0] - 20.5).abs() < 1e-4));
        assert!(matches!(&sn[1], Rec::Portal(p) if p.pos[0] == 3.0 && p.pos[2] == 2.5));
        let unit = snap_to_grid(&[area(10.3, 5.6, 20.3, 15.6)], 1.0);
        assert!(matches!(&unit[0], Rec::Mob(a) if a.min[0] == 10.0 && a.min[2] == 6.0));

        // same size as the first: width and depth of the first around each box's own centre
        let ss = same_size(&boxes).unwrap();
        assert_eq!(ss[0], boxes[0]);
        for x in &ss[1..] {
            let q = b(x);
            assert_eq!((q.2 - q.0, q.3 - q.1), (10.0, 10.0));
        }
        assert_eq!((b(&ss[1]).0 + b(&ss[1]).2) * 0.5, 32.0, "the centre stays");
        assert!(same_size(&[pt(1.0, 1.0), area(0.0, 0.0, 1.0, 1.0)]).is_none());
    }

    #[test]
    fn batch_helpers() {
        let mut a = MobArea { min: [0.0; 3], max: [10.0, 2.0, 10.0], mobs: vec![MobEntry { mob: 5, count: 1 }, MobEntry { mob: 6, count: 2 }] };
        assert!(mob_replace(&mut a, 5, 9) && a.mobs[0].mob == 9);
        assert!(!mob_replace(&mut a, 5, 9));
        assert!(mob_set_count(&mut a, Some(6), 4) && a.mobs[1].count == 4 && a.mobs[0].count == 1);
        assert!(mob_set_count(&mut a, None, 3) && a.mobs.iter().all(|e| e.count == 3));
        assert!(mob_remove(&mut a, 9) && a.mobs.len() == 1);
        mob_set_height(&mut a, 5.0);
        assert_eq!((a.min[1], a.max[1]), (0.0, 5.0));
        mob_grow(&mut a, 2.0);
        assert_eq!((a.min[0], a.max[0], a.min[2], a.max[2]), (-2.0, 12.0, -2.0, 12.0));
        mob_grow(&mut a, -100.0);
        assert!((a.max[0] - a.min[0] - 1.0).abs() < 1e-5, "a box cannot shrink past 1 unit");
        assert_eq!(common([3u32, 3, 3]), Some(3));
        assert_eq!(common([1u32, 2, 3]), None);
        assert_eq!(common(Vec::<u32>::new()), None);
    }

    #[test]
    fn clipboard_json_roundtrip() {
        let recs = vec![
            Rec::Mob(MobArea { min: [1.1, 0.25, 2.5], max: [5.7, 2.25, 9.9], mobs: vec![MobEntry { mob: 12, count: 3 }] }),
            Rec::Npc(NpcEntry { kind: 1, type_id: 7, points: vec![NpcPoint { pos: [3.3, 0.5, 4.4], dir: 1.5707964 }] }),
            Rec::Portal(Portal { pos: [1.0, 2.0, 3.0], country: 1, level_min: 5, level_max: 9, to_map: 35, to_pos: [10.1, 0.0, 20.2] }),
            Rec::Bind(Bind { country: 2, attr: 0, boxes: vec![Aabb { min: [0.0; 3], max: [3.0; 3] }] }),
            Rec::Region(Region { min: [0.0; 3], max: [8.0, 2.0, 8.0], kind: 2, b: 1, c: 2, d: 100 }),
            Rec::B([1.0, 2.0, 3.0]),
            Rec::A(shaiya_svmap::RawA { pos: [1.0, 2.0, 3.0], a: 7, b: 8 }),
        ];
        let text = clipboard_json(Some(35), &recs).unwrap();
        assert!(text.starts_with("{\"svmap-editor\":1,\"map\":35,\"records\":[{\"section\":\"mob_areas\""), "{text}");
        assert_eq!(parse_clipboard(&text).unwrap(), recs, "every float reads back exactly");
        assert_eq!(parse_clipboard(&clipboard_json(None, &recs[..1]).unwrap()).unwrap(), recs[..1].to_vec());
        for bad in ["", "hello", "{}", "{\"svmap-editor\":1,\"records\":[]}", "{\"svmap-editor\":1,\"records\":[{\"section\":\"x\",\"record\":{}}]}", "[1,2]"] {
            assert!(parse_clipboard(bad).is_none(), "{bad}");
        }
        let nan = vec![Rec::B([f32::NAN, 0.0, 0.0])];
        assert!(clipboard_json(None, &nan).is_none(), "NaN cannot travel as JSON");
        // hostile or damaged text: numbers out of f32 range, wrong types, deep nesting, huge numbers for u32 fields
        for bad in [
            "{\"svmap-editor\":1,\"records\":[{\"section\":\"b\",\"record\":[1e39,0,0]}]}",
            "{\"svmap-editor\":1,\"records\":[{\"section\":\"b\",\"record\":[\"x\",0,0]}]}",
            "{\"svmap-editor\":1,\"records\":[{\"section\":\"portals\",\"record\":{\"to_map\":-1}}]}",
            "{\"svmap-editor\":\"1\",\"records\":[{\"section\":\"b\",\"record\":[1,2,3]}]}",
            "{\"svmap-editor\":1,\"records\":{}}",
            &format!("{}{}", "[".repeat(10_000), "]".repeat(10_000)),
            &format!("{{\"svmap-editor\":1,\"records\":[{}]}}", "{".repeat(5_000)),
        ] {
            assert!(parse_clipboard(bad).is_none(), "{}", &bad[..bad.len().min(80)]);
        }
        let ok = parse_clipboard("  {\"svmap-editor\":1,\"records\":[{\"section\":\"b\",\"record\":[1,2,3]}]}  ").unwrap();
        assert_eq!(ok, vec![Rec::B([1.0, 2.0, 3.0])], "surrounding blanks are fine");
    }

    #[test]
    fn drop_slab() {
        let mut r = get(&map(), SectionId::MobAreas, 0).unwrap();
        drop_to_ground(&mut r, &|x, _| x * 10.0);
        match r {
            Rec::Mob(a) => {
                assert_eq!(a.min[1], 10.0);
                assert_eq!(a.max[1], 12.0);
            }
            _ => unreachable!(),
        }
    }
}
