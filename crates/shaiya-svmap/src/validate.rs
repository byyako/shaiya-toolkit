//! File-only and cross-file validation rules (see `docs/formats/svmap.md`). Client-aware rules live in `shaiya-svmap-tools`.

use serde::Serialize;

use crate::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Worth knowing.
    Info,
    /// Unlike every original file.
    Warning,
    /// The server loads it wrongly or a player cannot use it.
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionId {
    Size,
    Blocked,
    SectionA,
    SectionB,
    MobAreas,
    Npcs,
    Portals,
    Binds,
    Regions,
}

impl SectionId {
    pub fn label(self) -> &'static str {
        match self {
            SectionId::Size => "size",
            SectionId::Blocked => "blocked cells",
            SectionId::SectionA => "section A",
            SectionId::SectionB => "section B",
            SectionId::MobAreas => "monster areas",
            SectionId::Npcs => "NPCs",
            SectionId::Portals => "portals",
            SectionId::Binds => "bind areas",
            SectionId::Regions => "regions",
        }
    }
}

/// One validation result. `section` + `index` (+ `sub`, the point / box / entry inside the record) is the "select"
/// target for the editor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub section: SectionId,
    pub index: Option<usize>,
    pub sub: Option<usize>,
    /// Stable rule id, e.g. `pos-out-of-range`.
    pub code: &'static str,
    pub message: String,
}

/// Every rule code the file rules ([`validate`]) and the cross-file rules ([`validate_cross`]) can report. The client rules (which
/// need `.wld` and SData files) have their own list in the editor.
pub const RULE_CODES: &[&str] = &[
    "blocked-size",
    "non-finite",
    "pos-out-of-range",
    "region-out-of-range",
    "box-inverted",
    "area-empty",
    "area-count-zero",
    "area-count-high",
    "area-mob-zero",
    "area-on-blocked",
    "npc-no-points",
    "npc-type-invalid",
    "npc-type-zero",
    "npc-on-blocked",
    "portal-levels",
    "portal-country",
    "bind-no-box",
    "bind-country",
    "bind-on-blocked",
    "portal-map-missing",
    "portal-dest-outside",
    "portal-dest-blocked",
];

struct Out(Vec<Finding>);

impl Out {
    fn add(&mut self, severity: Severity, section: SectionId, index: Option<usize>, sub: Option<usize>, code: &'static str, message: String) {
        debug_assert!(RULE_CODES.contains(&code), "rule code {code} is missing from RULE_CODES");
        self.0.push(Finding { severity, section, index, sub, code, message });
    }
}

/// Every record that holds a NaN or infinite float, as (section, record).
pub fn non_finite_records(m: &Svmap) -> Vec<(SectionId, usize)> {
    let mut out = Vec::new();
    let bad = |v: &[f32]| v.iter().any(|x| !x.is_finite());
    for (i, a) in m.section_a.iter().enumerate() {
        if bad(&a.pos) {
            out.push((SectionId::SectionA, i));
        }
    }
    for (i, p) in m.points_b.iter().enumerate() {
        if bad(p) {
            out.push((SectionId::SectionB, i));
        }
    }
    for (i, a) in m.mob_areas.iter().enumerate() {
        if bad(&a.min) || bad(&a.max) {
            out.push((SectionId::MobAreas, i));
        }
    }
    for (i, n) in m.npcs.iter().enumerate() {
        if n.points.iter().any(|p| bad(&p.pos) || !p.dir.is_finite()) {
            out.push((SectionId::Npcs, i));
        }
    }
    for (i, p) in m.portals.iter().enumerate() {
        if bad(&p.pos) || bad(&p.to_pos) {
            out.push((SectionId::Portals, i));
        }
    }
    for (i, b) in m.binds.iter().enumerate() {
        if b.boxes.iter().any(|x| bad(&x.min) || bad(&x.max)) {
            out.push((SectionId::Binds, i));
        }
    }
    for (i, r) in m.regions.iter().enumerate() {
        if bad(&r.min) || bad(&r.max) {
            out.push((SectionId::Regions, i));
        }
    }
    out
}

fn in_map(size: u32, x: f32, z: f32) -> bool {
    let s = size as f32;
    x >= 0.0 && z >= 0.0 && x < s && z < s
}

/// Cell under a position, `None` when outside the map.
pub fn cell_of(size: u32, x: f32, z: f32) -> Option<(u32, u32)> {
    if in_map(size, x, z) {
        Some((x as u32, z as u32))
    } else {
        None
    }
}

fn blocked_at(m: &Svmap, pos: [f32; 3]) -> bool {
    match cell_of(m.size, pos[0], pos[2]) {
        Some((x, z)) => m.blocked.get(x, z),
        None => false,
    }
}

fn fmt3(v: [f32; 3]) -> String {
    format!("({}, {}, {})", v[0], v[1], v[2])
}

/// File-only rules.
pub fn validate(m: &Svmap) -> Vec<Finding> {
    use SectionId as S;
    use Severity::*;
    let mut o = Out(Vec::new());

    if m.blocked.size() != m.size {
        o.add(Error, S::Blocked, None, None, "blocked-size", format!("blocked grid is {0}x{0} but the map size is {1}", m.blocked.size(), m.size));
    }

    // Non-finite floats first: the position rules below compare, and NaN compares false.
    for (s, i) in &non_finite_records(m) {
        o.add(Error, *s, Some(*i), None, "non-finite", "contains NaN or infinity".to_string());
    }

    let pos_check = |o: &mut Out, s: SectionId, i: usize, sub: Option<usize>, what: &str, p: [f32; 3]| {
        if p.iter().all(|x| x.is_finite()) && !in_map(m.size, p[0], p[2]) {
            // Regions are not read further by the server and 4 original region corners (maps 1, 44 twice, 100) overhang the map edge, so
            // for them this is a warning.
            let (sev, code) = if s == S::Regions { (Warning, "region-out-of-range") } else { (Error, "pos-out-of-range") };
            o.add(sev, s, Some(i), sub, code, format!("{what} {} is outside the map (0 <= x, z < {})", fmt3(p), m.size));
        }
    };
    let box_check = |o: &mut Out, s: SectionId, i: usize, sub: Option<usize>, what: &str, min: [f32; 3], max: [f32; 3]| {
        if min.iter().chain(max.iter()).any(|x| !x.is_finite()) {
            return;
        }
        pos_check(o, s, i, sub, &format!("{what} corner"), min);
        pos_check(o, s, i, sub, &format!("{what} corner"), max);
        for (axis, name) in [(0, "x"), (1, "y"), (2, "z")] {
            if min[axis] > max[axis] {
                o.add(Error, s, Some(i), sub, "box-inverted", format!("{what}: min {name} {} is above max {name} {}", min[axis], max[axis]));
            }
        }
    };

    for (i, a) in m.section_a.iter().enumerate() {
        pos_check(&mut o, S::SectionA, i, None, "position", a.pos);
    }
    for (i, p) in m.points_b.iter().enumerate() {
        pos_check(&mut o, S::SectionB, i, None, "point", *p);
    }

    for (i, a) in m.mob_areas.iter().enumerate() {
        box_check(&mut o, S::MobAreas, i, None, "area", a.min, a.max);
        if a.mobs.is_empty() {
            o.add(Info, S::MobAreas, Some(i), None, "area-empty", "area has no monster entries".into());
        }
        for (k, e) in a.mobs.iter().enumerate() {
            if e.count == 0 {
                o.add(Error, S::MobAreas, Some(i), Some(k), "area-count-zero", format!("entry {k}: count is 0"));
            } else if e.count > 8 {
                o.add(Warning, S::MobAreas, Some(i), Some(k), "area-count-high", format!("entry {k}: count {} is above 8 (the originals stop at 8)", e.count));
            }
            if e.mob == 0 {
                o.add(Warning, S::MobAreas, Some(i), Some(k), "area-mob-zero", format!("entry {k}: monster id 0"));
            }
        }
        if a.min.iter().chain(a.max.iter()).all(|x| x.is_finite()) && blocked_at(m, a.centre()) {
            o.add(Warning, S::MobAreas, Some(i), None, "area-on-blocked", "area centre is on a blocked cell".into());
        }
    }

    for (i, n) in m.npcs.iter().enumerate() {
        if n.points.is_empty() {
            o.add(Error, S::Npcs, Some(i), None, "npc-no-points", "NPC has no points".into());
        }
        if n.kind > 13 {
            o.add(Error, S::Npcs, Some(i), None, "npc-type-invalid", format!("NPC type {} is above 13", n.kind));
        } else if n.kind == 0 {
            o.add(Info, S::Npcs, Some(i), None, "npc-type-zero", format!("NPC type 0 (type id {})", n.type_id));
        }
        for (k, p) in n.points.iter().enumerate() {
            pos_check(&mut o, S::Npcs, i, Some(k), &format!("point {k}"), p.pos);
        }
        if let Some(p) = n.points.first() {
            if blocked_at(m, p.pos) {
                o.add(Warning, S::Npcs, Some(i), Some(0), "npc-on-blocked", "NPC's first point is on a blocked cell".into());
            }
        }
    }

    for (i, p) in m.portals.iter().enumerate() {
        pos_check(&mut o, S::Portals, i, None, "portal position", p.pos);
        if p.level_max != 0 && p.level_min > p.level_max {
            o.add(Warning, S::Portals, Some(i), None, "portal-levels", format!("level min {} is above level max {}", p.level_min, p.level_max));
        }
        if p.country > 2 && p.country < 1000 {
            o.add(Warning, S::Portals, Some(i), None, "portal-country", format!("country {} is not 0, 1, 2 or an instance id (1000+)", p.country));
        }
    }

    for (i, b) in m.binds.iter().enumerate() {
        if b.boxes.is_empty() {
            o.add(Error, S::Binds, Some(i), None, "bind-no-box", "bind area has no box".into());
        }
        if b.country > 3 {
            o.add(Warning, S::Binds, Some(i), None, "bind-country", format!("country {} is above 3", b.country));
        }
        for (k, x) in b.boxes.iter().enumerate() {
            box_check(&mut o, S::Binds, i, Some(k), &format!("box {k}"), x.min, x.max);
            if x.min.iter().chain(x.max.iter()).all(|v| v.is_finite()) && blocked_at(m, x.centre()) {
                o.add(Warning, S::Binds, Some(i), Some(k), "bind-on-blocked", format!("box {k} centre is on a blocked cell"));
            }
        }
    }

    for (i, r) in m.regions.iter().enumerate() {
        box_check(&mut o, S::Regions, i, None, "region", r.min, r.max);
    }

    o.0
}

/// Cross-file rules for the map `map_id`. `lookup` returns the other maps of the open folder (it may return the map
/// itself too, for portals that stay on their map).
pub fn validate_cross<'a>(m: &Svmap, lookup: &dyn Fn(u32) -> Option<&'a Svmap>) -> Vec<Finding> {
    use SectionId as S;
    use Severity::*;
    let mut o = Out(Vec::new());
    for (i, p) in m.portals.iter().enumerate() {
        // Map 200 is the guild ranking (no file); instance ids are portal countries, not destinations.
        if p.to_map == 200 {
            continue;
        }
        match lookup(p.to_map) {
            None => o.add(Warning, S::Portals, Some(i), None, "portal-map-missing", format!("destination map {} is not in the folder", p.to_map)),
            Some(dest) => {
                let to = p.to_pos;
                if !to.iter().all(|x| x.is_finite()) {
                    continue;
                }
                if !in_map(dest.size, to[0], to[2]) {
                    o.add(Error, S::Portals, Some(i), None, "portal-dest-outside", format!("destination {} is outside map {} (size {})", fmt3(to), p.to_map, dest.size));
                } else if blocked_at(dest, to) {
                    o.add(Warning, S::Portals, Some(i), None, "portal-dest-blocked", format!("destination {} on map {} is a blocked cell", fmt3(to), p.to_map));
                }
            }
        }
    }
    o.0
}

/// Counts of findings per severity: (errors, warnings, infos).
pub fn tally(findings: &[Finding]) -> (usize, usize, usize) {
    let mut t = (0, 0, 0);
    for f in findings {
        match f.severity {
            Severity::Error => t.0 += 1,
            Severity::Warning => t.1 += 1,
            Severity::Info => t.2 += 1,
        }
    }
    t
}
