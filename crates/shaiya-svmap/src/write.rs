//! Writer for `.svmap` files: the exact inverse of [`crate::read::read`]. Floats are copied bit for bit.

use crate::model::*;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn vec3(&mut self, v: [f32; 3]) {
        for x in v {
            self.f32(x);
        }
    }
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("record count fits u32"));
    }
}

/// Serialises a map. Each blocked-grid byte is written as stored, so a map read from a file and written back
/// unchanged gives the same bytes.
pub fn write(m: &Svmap) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(m.blocked.as_bytes().len() + 4096));
    w.u32(m.size);
    w.0.extend_from_slice(m.blocked.as_bytes());

    w.len(m.section_a.len());
    for a in &m.section_a {
        w.vec3(a.pos);
        w.u32(a.a);
        w.u32(a.b);
    }

    w.len(m.points_b.len());
    for p in &m.points_b {
        w.vec3(*p);
    }

    w.len(m.mob_areas.len());
    for a in &m.mob_areas {
        w.vec3(a.min);
        w.vec3(a.max);
        w.len(a.mobs.len());
        for e in &a.mobs {
            w.u32(e.mob);
            w.u32(e.count);
        }
    }

    w.len(m.npcs.len());
    for n in &m.npcs {
        w.u32(n.kind);
        w.u32(n.type_id);
        w.len(n.points.len());
        for p in &n.points {
            w.vec3(p.pos);
            w.f32(p.dir);
        }
    }

    w.len(m.portals.len());
    for p in &m.portals {
        w.vec3(p.pos);
        w.u32(p.country);
        w.u16(p.level_min);
        w.u16(p.level_max);
        w.u32(p.to_map);
        w.vec3(p.to_pos);
    }

    w.len(m.binds.len());
    for b in &m.binds {
        w.len(b.boxes.len());
        w.u32(b.country);
        w.u32(b.attr);
        for bx in &b.boxes {
            w.vec3(bx.min);
            w.vec3(bx.max);
        }
    }

    w.len(m.regions.len());
    for r in &m.regions {
        w.vec3(r.min);
        w.vec3(r.max);
        w.u32(r.kind);
        w.u8(r.b);
        w.u8(r.c);
        w.u16(r.d);
    }

    w.0
}
