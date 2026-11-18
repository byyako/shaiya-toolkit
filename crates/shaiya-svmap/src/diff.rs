//! Record-level difference between two maps: what was added, removed or changed per section.
//!
//! Records are compared bit for bit (through their `Debug` text, which prints the shortest decimal of each float and so
//! tells `-0.0` from `0.0`). Identical leading and trailing records are skipped, records present on both sides in the
//! middle are matched by value (a moved record is a change, not a remove plus an add), and what is left on both sides
//! is paired in order as "changed", the surplus being "removed" or "added".

use std::collections::HashMap;

use serde::Serialize;

use crate::model::*;
use crate::validate::SectionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Added,
    Removed,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiffEntry {
    pub section: SectionId,
    pub kind: DiffKind,
    /// Index in `a` for removed/changed, in `b` for added.
    pub index: usize,
    /// Index in `b` for changed records.
    pub index_b: Option<usize>,
    pub text: String,
}

fn f(v: f32) -> String {
    format!("{v}")
}

fn p3(v: [f32; 3]) -> String {
    format!("({}, {}, {})", f(v[0]), f(v[1]), f(v[2]))
}

fn describe_area(a: &MobArea) -> String {
    let mobs: Vec<String> = a.mobs.iter().map(|e| format!("{}x{}", e.mob, e.count)).collect();
    format!("{} - {} [{}]", p3(a.min), p3(a.max), mobs.join(", "))
}

fn describe_npc(n: &NpcEntry) -> String {
    let pts: Vec<String> = n.points.iter().map(|p| format!("{} dir {}", p3(p.pos), f(p.dir))).collect();
    format!("type {} id {} at {}", n.kind, n.type_id, pts.join(" -> "))
}

fn describe_portal(p: &Portal) -> String {
    format!(
        "{} country {} lv {}-{} -> map {} {}",
        p3(p.pos),
        p.country,
        p.level_min,
        p.level_max,
        p.to_map,
        p3(p.to_pos)
    )
}

fn describe_bind(b: &Bind) -> String {
    let boxes: Vec<String> = b.boxes.iter().map(|x| format!("{} - {}", p3(x.min), p3(x.max))).collect();
    format!("country {} attr {} [{}]", b.country, b.attr, boxes.join("; "))
}

fn describe_region(r: &Region) -> String {
    format!("{} - {} kind {} tail ({}, {}, {})", p3(r.min), p3(r.max), r.kind, r.b, r.c, r.d)
}

fn describe_a(a: &RawA) -> String {
    format!("{} a {} b {}", p3(a.pos), a.a, a.b)
}

fn diff_list<T: std::fmt::Debug>(
    section: SectionId,
    a: &[T],
    b: &[T],
    describe: &dyn Fn(&T) -> String,
    out: &mut Vec<DiffEntry>,
) {
    let key = |t: &T| format!("{t:?}");
    let ka: Vec<String> = a.iter().map(key).collect();
    let kb: Vec<String> = b.iter().map(key).collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && ka[start] == kb[start] {
        start += 1;
    }
    let (mut ea, mut eb) = (a.len(), b.len());
    while ea > start && eb > start && ka[ea - 1] == kb[eb - 1] {
        ea -= 1;
        eb -= 1;
    }
    // Match equal records inside the middle by value.
    let mut pool: HashMap<&str, Vec<usize>> = HashMap::new();
    for j in start..eb {
        pool.entry(kb[j].as_str()).or_default().push(j);
    }
    for v in pool.values_mut() {
        v.reverse(); // pop() takes the earliest
    }
    let mut left_a = Vec::new();
    for i in start..ea {
        match pool.get_mut(ka[i].as_str()).and_then(|v| v.pop()) {
            Some(_) => {}
            None => left_a.push(i),
        }
    }
    let mut left_b: Vec<usize> = pool.into_values().flatten().collect();
    left_b.sort_unstable();
    let paired = left_a.len().min(left_b.len());
    for k in 0..paired {
        let (i, j) = (left_a[k], left_b[k]);
        out.push(DiffEntry {
            section,
            kind: DiffKind::Changed,
            index: i,
            index_b: Some(j),
            text: format!("{}  =>  {}", describe(&a[i]), describe(&b[j])),
        });
    }
    for &i in &left_a[paired..] {
        out.push(DiffEntry { section, kind: DiffKind::Removed, index: i, index_b: None, text: describe(&a[i]) });
    }
    for &j in &left_b[paired..] {
        out.push(DiffEntry { section, kind: DiffKind::Added, index: j, index_b: None, text: describe(&b[j]) });
    }
}

/// Differences from `a` to `b`, grouped by section in file order.
pub fn diff(a: &Svmap, b: &Svmap) -> Vec<DiffEntry> {
    let mut out = Vec::new();
    if a.size != b.size {
        out.push(DiffEntry {
            section: SectionId::Size,
            kind: DiffKind::Changed,
            index: 0,
            index_b: None,
            text: format!("{}  =>  {}", a.size, b.size),
        });
    }
    if a.blocked != b.blocked {
        let text = match a.blocked.differing_cells(&b.blocked) {
            Some(n) => format!("{n} cells differ ({} -> {} blocked)", a.blocked.count(), b.blocked.count()),
            None => format!("grid {0}x{0} -> {1}x{1}", a.blocked.size(), b.blocked.size()),
        };
        out.push(DiffEntry { section: SectionId::Blocked, kind: DiffKind::Changed, index: 0, index_b: None, text });
    }
    diff_list(SectionId::SectionA, &a.section_a, &b.section_a, &describe_a, &mut out);
    diff_list(SectionId::SectionB, &a.points_b, &b.points_b, &|p| p3(*p), &mut out);
    diff_list(SectionId::MobAreas, &a.mob_areas, &b.mob_areas, &describe_area, &mut out);
    diff_list(SectionId::Npcs, &a.npcs, &b.npcs, &describe_npc, &mut out);
    diff_list(SectionId::Portals, &a.portals, &b.portals, &describe_portal, &mut out);
    diff_list(SectionId::Binds, &a.binds, &b.binds, &describe_bind, &mut out);
    diff_list(SectionId::Regions, &a.regions, &b.regions, &describe_region, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f32) -> MobArea {
        MobArea { min: [x, 0.0, 0.0], max: [x + 1.0, 2.0, 1.0], mobs: vec![MobEntry { mob: 1, count: 1 }] }
    }

    #[test]
    fn identical_maps_have_no_diff() {
        let mut a = Svmap::new(64);
        a.mob_areas.push(area(1.0));
        assert!(diff(&a, &a.clone()).is_empty());
    }

    #[test]
    fn added_removed_changed() {
        let mut a = Svmap::new(64);
        a.mob_areas = vec![area(1.0), area(2.0), area(3.0)];
        let mut b = a.clone();
        b.mob_areas[1] = area(9.0); // changed
        b.mob_areas.push(area(4.0)); // added
        let d = diff(&a, &b);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].kind, DiffKind::Changed);
        assert_eq!((d[0].index, d[0].index_b), (1, Some(1)));
        assert_eq!(d[1].kind, DiffKind::Added);
        assert_eq!(d[1].index, 3);
        let r = diff(&b, &a);
        assert!(r.iter().any(|e| e.kind == DiffKind::Removed && e.index == 3));
    }

    #[test]
    fn an_insertion_does_not_shift_everything() {
        let mut a = Svmap::new(64);
        a.mob_areas = (0..20).map(|i| area(i as f32)).collect();
        let mut b = a.clone();
        b.mob_areas.insert(5, area(99.0));
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!((d[0].kind, d[0].index), (DiffKind::Added, 5));
    }

    #[test]
    fn blocked_and_size_changes() {
        let a = Svmap::new(64);
        let mut b = a.clone();
        b.blocked.set(3, 3, true);
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].section, SectionId::Blocked);
        assert!(d[0].text.starts_with("1 cells differ"));
        let c = Svmap::new(32);
        let d = diff(&a, &c);
        assert!(d.iter().any(|e| e.section == SectionId::Size));
    }
}
