//! The Measure tool's maths and state (pure, no egui): a polyline of ground points with per-segment lengths, the bearing of the last
//! segment, the closed polygon's area, a circle for radius mode, snapping to record anchors, and the text the tool copies.
//!
//! World x runs east and z north, so a bearing of 0 degrees is north (+z) and 90 degrees is east (+x), clockwise as on a compass.

use shaiya_svmap::Svmap;

/// The radius a ring marks as the portal range (`client_rules::PORTAL_USE_RANGE`), drawn faintly while the circle is close to it.
pub const RING: f32 = crate::client_rules::PORTAL_USE_RANGE;
/// How close a circle's radius is to [`RING`] before the ring shows.
pub const RING_SLACK: f32 = 2.0;
/// How far, in screen pixels, a point snaps to a record anchor or the first point closes the polygon.
pub const SNAP_PX: f32 = 8.0;

/// Segment lengths and their sum for a chain of points (x, z).
pub fn lengths(points: &[[f32; 2]]) -> (Vec<f32>, f32) {
    let segs: Vec<f32> = points.windows(2).map(|w| dist(w[0], w[1])).collect();
    let total = segs.iter().sum();
    (segs, total)
}

pub fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// The compass bearing of the step from `a` to `b` in degrees (0 = north, 90 = east, 0 to 360).
pub fn bearing(a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = (b[0] - a[0]).atan2(b[1] - a[1]).to_degrees();
    let d = if d < 0.0 { d + 360.0 } else { d };
    if d >= 360.0 {
        0.0
    } else {
        d
    }
}

/// The area of the polygon through `points` (shoelace formula, absolute value).
pub fn polygon_area(points: &[[f32; 2]]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        s += a[0] as f64 * b[1] as f64 - b[0] as f64 * a[1] as f64;
    }
    (s.abs() * 0.5) as f32
}

/// Moves `b` so the step from `a` has a bearing that is a multiple of `step_deg` (the length stays).
pub fn snap_angle(a: [f32; 2], b: [f32; 2], step_deg: f32) -> [f32; 2] {
    let d = dist(a, b);
    if d < 1e-4 || step_deg <= 0.0 {
        return b;
    }
    let ang = (bearing(a, b) / step_deg).round() * step_deg;
    let r = ang.to_radians();
    [a[0] + r.sin() * d, a[1] + r.cos() * d]
}

/// How many unit cells of a map of `size` have their centre inside the polygon (even-odd rule).
pub fn cells_inside(points: &[[f32; 2]], size: u32) -> u32 {
    if points.len() < 3 {
        return 0;
    }
    let s = size as f32;
    let (mut x0, mut x1, mut z0, mut z1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for p in points {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        z0 = z0.min(p[1]);
        z1 = z1.max(p[1]);
    }
    let (x0, x1, z0, z1) = (x0.clamp(0.0, s).floor() as u32, x1.clamp(0.0, s).ceil() as u32, z0.clamp(0.0, s).floor() as u32, z1.clamp(0.0, s).ceil() as u32);
    let mut n = 0;
    for z in z0..z1.min(size) {
        for x in x0..x1.min(size) {
            if inside(points, [x as f32 + 0.5, z as f32 + 0.5]) {
                n += 1;
            }
        }
    }
    n
}

fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

/// The length of an NPC's walking path along its points (horizontal distance, like the Measure tool).
pub fn path_length(points: &[[f32; 3]]) -> f32 {
    points.windows(2).map(|w| dist([w[0][0], w[0][2]], [w[1][0], w[1][2]])).sum()
}

/// The inspector's line for an NPC path: "Path 84.2 units (5 points)". `None` for fewer than 2 points.
pub fn path_text(points: &[[f32; 3]]) -> Option<String> {
    (points.len() >= 2).then(|| format!("Path {:.1} units ({} points)", path_length(points), points.len()))
}

/// Calls `f` with every place a measured point snaps to: the corners and the centre of every box (monster areas, bind boxes,
/// regions), NPC points and portal positions.
pub fn each_anchor(map: &Svmap, f: &mut dyn FnMut([f32; 2])) {
    let mut boxed = |min: [f32; 3], max: [f32; 3]| {
        let (x0, x1, z0, z1) = (min[0].min(max[0]), min[0].max(max[0]), min[2].min(max[2]), min[2].max(max[2]));
        for p in [[x0, z0], [x1, z0], [x0, z1], [x1, z1], [(x0 + x1) * 0.5, (z0 + z1) * 0.5]] {
            f(p);
        }
    };
    for a in &map.mob_areas {
        boxed(a.min, a.max);
    }
    for g in &map.regions {
        boxed(g.min, g.max);
    }
    for b in &map.binds {
        for x in &b.boxes {
            boxed(x.min, x.max);
        }
    }
    for n in &map.npcs {
        for p in &n.points {
            f([p.pos[0], p.pos[2]]);
        }
    }
    for p in &map.portals {
        f([p.pos[0], p.pos[2]]);
    }
}

/// The anchors of a map as a list.
#[cfg(test)]
pub fn anchors(map: &Svmap) -> Vec<[f32; 2]> {
    let mut v = Vec::new();
    each_anchor(map, &mut |p| v.push(p));
    v
}

/// The anchor of the map nearest to `p` within `px` screen pixels at `zoom` pixels per unit.
pub fn snap_in_map(map: &Svmap, p: [f32; 2], zoom: f32, px: f32) -> Option<[f32; 2]> {
    let max = px / zoom.max(1e-6);
    let mut best: Option<(f32, [f32; 2])> = None;
    each_anchor(map, &mut |a| {
        let d = dist(a, p);
        if d <= max && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, a));
        }
    });
    best.map(|(_, a)| a)
}

/// One measurement: a chain of points (a drag makes two), or a circle in radius mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measure {
    pub points: Vec<[f32; 2]>,
    /// No more points are added: the next click starts a new measurement.
    pub done: bool,
    /// The polygon is closed (the first point was clicked again).
    pub closed: bool,
    /// Cells inside the closed polygon.
    pub cells: u32,
    /// Radius mode: the centre and the radius.
    pub circle: Option<([f32; 2], f32)>,
}

impl Measure {
    pub fn active(&self) -> bool {
        !self.points.is_empty() || self.circle.is_some()
    }

    pub fn clear(&mut self) {
        *self = Measure::default();
    }

    /// A click: the first point of a new measurement, or the next point of the chain.
    pub fn click(&mut self, p: [f32; 2]) {
        if self.done || self.circle.is_some() {
            self.clear();
        }
        if self.points.last() != Some(&p) {
            self.points.push(p);
        }
    }

    /// A drag from `a` to `b` is one finished segment.
    pub fn segment(&mut self, a: [f32; 2], b: [f32; 2]) {
        self.clear();
        self.points = vec![a, b];
        self.done = false;
    }

    /// A drag in radius mode.
    pub fn circle_to(&mut self, centre: [f32; 2], to: [f32; 2]) {
        self.points.clear();
        self.done = true;
        self.closed = false;
        self.circle = Some((centre, dist(centre, to)));
    }

    pub fn finish(&mut self) {
        if !self.points.is_empty() {
            self.done = true;
        }
    }

    /// Clicking the first point of a chain of at least 3 points closes the polygon.
    pub fn close(&mut self, size: u32) {
        if self.points.len() >= 3 {
            self.closed = true;
            self.done = true;
            self.cells = cells_inside(&self.points, size);
        }
    }

    /// Backspace: takes the last point back (and opens a closed polygon).
    pub fn pop(&mut self) {
        if self.circle.is_some() {
            self.clear();
            return;
        }
        if self.closed {
            self.closed = false;
            self.cells = 0;
        }
        self.points.pop();
        self.done = false;
        if self.points.is_empty() {
            self.clear();
        }
    }

    /// The points to draw and measure: a closed polygon repeats the first point at the end.
    pub fn chain(&self) -> Vec<[f32; 2]> {
        let mut v = self.points.clone();
        if self.closed {
            if let Some(f) = v.first().copied() {
                v.push(f);
            }
        }
        v
    }

    pub fn total(&self) -> f32 {
        lengths(&self.chain()).1
    }

    pub fn area(&self) -> Option<f32> {
        self.closed.then(|| polygon_area(&self.points))
    }

    /// The text Ctrl+C copies: "length 123.4 (3 segments), area 812.0", or "radius 40.0" for a circle.
    pub fn copy_text(&self) -> Option<String> {
        if let Some((_, r)) = self.circle {
            return Some(format!("radius {r:.1}"));
        }
        let chain = self.chain();
        if chain.len() < 2 {
            return None;
        }
        let (segs, total) = lengths(&chain);
        let mut s = format!("length {total:.1} ({} segment{})", segs.len(), if segs.len() == 1 { "" } else { "s" });
        if let Some(a) = self.area() {
            s.push_str(&format!(", area {a:.1}"));
        }
        Some(s)
    }

    /// The summary line at the end of the chain: total, the differences first to last and the last bearing. `ground` gives the
    /// height at a point (the dy part).
    pub fn summary(&self, ground: &dyn Fn(f32, f32) -> f32) -> Option<String> {
        let chain = self.chain();
        if chain.len() < 2 {
            return None;
        }
        let (a, b) = (chain[0], *chain.last()?);
        let last_from = chain[chain.len() - 2];
        let dy = ground(b[0], b[1]) - ground(a[0], a[1]);
        let mut s = format!("total {:.1}  dx {:.1}  dz {:.1}  dy {:.1}  {:.0} deg", lengths(&chain).1, b[0] - a[0], b[1] - a[1], dy, bearing(last_from, b));
        if let Some(area) = self.area() {
            s.push_str(&format!("\narea {area:.1} unit2, {} cells", self.cells));
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_math() {
        // lengths: a 3-4-5 step then 10 east
        let (segs, total) = lengths(&[[0.0, 0.0], [3.0, 4.0], [13.0, 4.0]]);
        assert_eq!(segs, vec![5.0, 10.0]);
        assert_eq!(total, 15.0);
        assert_eq!(lengths(&[[1.0, 1.0]]), (vec![], 0.0));
        // bearing: north is 0, east 90, south 180, west 270
        assert_eq!(bearing([0.0, 0.0], [0.0, 5.0]), 0.0);
        assert!((bearing([0.0, 0.0], [5.0, 0.0]) - 90.0).abs() < 1e-4);
        assert!((bearing([0.0, 0.0], [0.0, -5.0]) - 180.0).abs() < 1e-4);
        assert!((bearing([0.0, 0.0], [-5.0, 0.0]) - 270.0).abs() < 1e-4);
        assert!((bearing([0.0, 0.0], [1.0, 1.0]) - 45.0).abs() < 1e-3);
        // area: a 10 x 10 square (either winding), an L shape of 3 unit squares, fewer than 3 points
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(polygon_area(&sq), 100.0);
        let rev: Vec<[f32; 2]> = sq.iter().rev().copied().collect();
        assert_eq!(polygon_area(&rev), 100.0);
        let l = [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0], [1.0, 2.0], [0.0, 2.0]];
        assert_eq!(polygon_area(&l), 3.0);
        assert_eq!(polygon_area(&sq[..2]), 0.0);
        // cells: the square covers 100 cells, the L shape 3
        assert_eq!(cells_inside(&sq, 64), 100);
        assert_eq!(cells_inside(&l, 64), 3);
        // 15 degree snap: 20 degrees goes to 15, 40 to 45, the length stays
        let a = [10.0, 10.0];
        let b = [a[0] + 10.0 * 20f32.to_radians().sin(), a[1] + 10.0 * 20f32.to_radians().cos()];
        let s = snap_angle(a, b, 15.0);
        assert!((bearing(a, s) - 15.0).abs() < 1e-2, "{}", bearing(a, s));
        assert!((dist(a, s) - 10.0).abs() < 1e-3);
        let c = [a[0] + 7.0 * 40f32.to_radians().sin(), a[1] + 7.0 * 40f32.to_radians().cos()];
        assert!((bearing(a, snap_angle(a, c, 15.0)) - 45.0).abs() < 1e-2);
        assert_eq!(snap_angle(a, a, 15.0), a, "a zero step stays");
    }

    #[test]
    fn path_length_and_text() {
        let pts = [[0.0, 0.0, 0.0], [30.0, 5.0, 40.0], [30.0, 0.0, 90.0]];
        assert_eq!(path_length(&pts), 100.0);
        assert_eq!(path_text(&pts).as_deref(), Some("Path 100.0 units (3 points)"));
        assert_eq!(path_text(&pts[..1]), None);
    }

    #[test]
    fn anchors_and_snapping() {
        use shaiya_svmap::{MobArea, NpcEntry, NpcPoint};
        let mut m = Svmap::new(128);
        m.mob_areas.push(MobArea { min: [10.0, 0.0, 20.0], max: [30.0, 2.0, 40.0], mobs: vec![] });
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [60.0, 0.0, 70.0], dir: 0.0 }] });
        let a = anchors(&m);
        assert_eq!(a.len(), 6, "5 per box and 1 NPC point");
        assert!(a.contains(&[20.0, 30.0]), "box centre");
        // 8 px at 4 px per unit is 2 units
        assert_eq!(snap_in_map(&m, [11.0, 21.0], 4.0, 8.0), Some([10.0, 20.0]));
        assert_eq!(snap_in_map(&m, [13.0, 21.0], 4.0, 8.0), None);
        assert_eq!(snap_in_map(&m, [59.0, 71.0], 4.0, 8.0), Some([60.0, 70.0]));
    }

    #[test]
    fn measurement_state_and_texts() {
        let mut m = Measure::default();
        assert!(!m.active() && m.copy_text().is_none());
        m.click([0.0, 0.0]);
        m.click([30.0, 40.0]);
        m.click([30.0, 40.0]); // the second click of a double click adds nothing
        m.click([60.0, 40.0]);
        assert_eq!(m.points.len(), 3);
        assert_eq!(m.copy_text().as_deref(), Some("length 80.0 (2 segments)"));
        let s = m.summary(&|x, _| x * 0.1).unwrap();
        assert!(s.starts_with("total 80.0  dx 60.0  dz 40.0  dy 6.0  90 deg"), "{s}");
        // closing makes a polygon: the chain goes back to the start
        m.close(128);
        assert!(m.closed && m.done);
        assert_eq!(m.area(), Some(600.0));
        assert!((m.cells as i32 - 600).abs() <= 40, "{}", m.cells);
        assert_eq!(m.copy_text().as_deref(), Some("length 152.1 (3 segments), area 600.0"));
        // a click after a finished measurement starts a new one
        m.click([5.0, 5.0]);
        assert_eq!(m.points, vec![[5.0, 5.0]]);
        assert!(!m.closed);
        // Backspace takes points back and clears at the end
        m.click([9.0, 9.0]);
        m.pop();
        assert_eq!(m.points.len(), 1);
        m.pop();
        assert!(!m.active());
        // a circle
        m.circle_to([10.0, 10.0], [10.0, 50.0]);
        assert_eq!(m.circle, Some(([10.0, 10.0], 40.0)));
        assert_eq!(m.copy_text().as_deref(), Some("radius 40.0"));
        m.click([1.0, 1.0]);
        assert!(m.circle.is_none() && m.points.len() == 1);
    }
}
