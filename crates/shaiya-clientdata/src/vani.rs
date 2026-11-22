//! `.VANI` vertex-animated models (`Entity/...`): a model whose animation is a full set of vertices for every step. Layout as observed in
//! client files (little-endian, no header; a file is read only when it ends exactly after the last field):
//!
//! ```text
//! f32 sphere centre[3]; f32 sphere radius; f32 box min[3], max[3]
//! i32 item count; i32 step count; i32 frame_ms                       (frame_ms: inferred, milliseconds per step)
//! per item:  i32 length + texture bytes (raw, NUL included)
//!            i32 face count; faces (3 x u16, shared by every step)
//!            i32 vertices per step
//!            vertices: 36 bytes x vertices per step x step count, step by step
//! f32 second box min[3], max[3]
//! i32 collision flag             0, or 1 followed by a collision mesh: i32 point count, f32[3] points, i32 face count, faces
//! vertex (36 bytes):  f32 position[3]; f32 normal[3]; u32 diffuse colour; f32 u, v
//! ```
//!
//! Floats are kept as stored and texture names as raw bytes (including the NUL and anything after it), so [`Vani::to_bytes`] gives back
//! the bytes [`Vani::parse`] read. A collision flag other than 0 or 1 is refused (no file is known to use the collision branch, so
//! only the layout the `.smod` collision mesh has is read, once). The meaning of `frame_ms` is inferred and was not checked in the game.

use crate::error::{decode_cp949, Cur, Error, Result};

/// One vertex, 36 bytes: position, normal, diffuse colour (raw `u32`), texture coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct VaniVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub diffuse: u32,
    pub uv: [f32; 2],
}

/// One item: a texture, the faces shared by every step and the vertex sets of all steps.
#[derive(Clone, Debug, PartialEq)]
pub struct VaniItem {
    /// The texture name as stored: raw bytes, the NUL and anything after it included (may be empty).
    pub texture: Vec<u8>,
    pub faces: Vec<[u16; 3]>,
    pub verts_per_step: usize,
    /// `verts_per_step` vertices for step 0, then for step 1, and so on: `verts_per_step * step_count` in all.
    pub vertices: Vec<VaniVertex>,
}

/// The collision mesh of the `.smod` layout: points and faces, no texture.
#[derive(Clone, Debug, PartialEq)]
pub struct VaniCollision {
    pub points: Vec<[f32; 3]>,
    pub faces: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vani {
    pub centre: [f32; 3],
    pub radius: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub step_count: usize,
    /// Inferred: milliseconds per step (33, 66 or 100 on US EP4.5).
    pub frame_ms: i32,
    pub items: Vec<VaniItem>,
    pub box2: ([f32; 3], [f32; 3]),
    /// Present when the collision flag is 1 (no file of US EP4.5 has one).
    pub collision: Option<VaniCollision>,
}

const VERTEX: usize = 36;
const FACE: usize = 6;
const ITEM_LIMIT: usize = 4096;
const STEP_LIMIT: usize = 1 << 16;
const COUNT_LIMIT: usize = 1 << 24;
const TEXTURE_LIMIT: usize = 1 << 20;

fn faces(r: &mut Cur, what: &str) -> Result<Vec<[u16; 3]>> {
    let n = r.count(FACE, COUNT_LIMIT, what)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push([r.u16(what)?, r.u16(what)?, r.u16(what)?]);
    }
    Ok(out)
}

impl Vani {
    /// Reads a whole `.VANI`; the file must end exactly after the collision flag (and mesh).
    pub fn parse(data: &[u8], file: &str) -> Result<Vani> {
        let mut r = Cur::new(data, file);
        let centre = r.f3("sphere centre")?;
        let radius = r.f32("sphere radius")?;
        let (min, max) = (r.f3("box min")?, r.f3("box max")?);
        // an item is at least a texture length, a face count and a vertex count
        let n_items = r.count(12, ITEM_LIMIT, "item count")?;
        let step_count = r.count(0, STEP_LIMIT, "step count")?;
        let frame_ms = r.i32("frame_ms")?;
        let mut items = Vec::with_capacity(n_items);
        for _ in 0..n_items {
            let texture = r.raw_str_i32("texture name")?.to_vec();
            let faces = faces(&mut r, "faces")?;
            let verts_per_step = r.count(0, COUNT_LIMIT, "vertices per step")?;
            let total = verts_per_step.checked_mul(step_count).filter(|t| t.checked_mul(VERTEX).is_some_and(|b| b <= r.left()));
            let Some(total) = total else {
                return Err(r.fail(format!("vertices: {verts_per_step} per step x {step_count} steps does not fit in the file")));
            };
            let mut vertices = Vec::with_capacity(total);
            for _ in 0..total {
                let pos = r.f3("vertex position")?;
                let normal = r.f3("vertex normal")?;
                let diffuse = r.u32("vertex colour")?;
                let uv = [r.f32("vertex u")?, r.f32("vertex v")?];
                vertices.push(VaniVertex { pos, normal, diffuse, uv });
            }
            items.push(VaniItem { texture, faces, verts_per_step, vertices });
        }
        let box2 = (r.f3("second box min")?, r.f3("second box max")?);
        let flag_at = r.p;
        let flag = r.i32("collision flag")?;
        let collision = match flag {
            0 => None,
            1 => {
                let np = r.count(12, COUNT_LIMIT, "collision point count")?;
                let mut points = Vec::with_capacity(np);
                for _ in 0..np {
                    points.push(r.f3("collision point")?);
                }
                Some(VaniCollision { points, faces: faces(&mut r, "collision faces")? })
            }
            other => {
                r.p = flag_at;
                return Err(r.fail(format!("collision flag {other}: only 0 and 1 are known")));
            }
        };
        r.end("model")?;
        Ok(Vani { centre, radius, min, max, step_count, frame_ms, items, box2, collision })
    }

    /// The bytes of this model; for a parsed file, exactly the bytes that were read.
    ///
    /// Panics when the model cannot be written (see [`Vani::try_to_bytes`]), which no parsed file can hold.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("model cannot be written")
    }

    /// Like [`Vani::to_bytes`], but refuses a model that does not fit the file: vertices that are not `vertices per step x step count`,
    /// a count above what the reader accepts. The bytes are read back with [`Vani::parse`] and refused unless they read.
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let bad = |what: String| Error::Format { file: "model".into(), what, offset: 0 };
        if self.items.len() > ITEM_LIMIT || self.step_count > STEP_LIMIT {
            return Err(bad("too many items or steps".into()));
        }
        let mut d = Vec::new();
        let mut put = |v: &[f32]| {
            for c in v {
                d.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        };
        put(&self.centre);
        put(&[self.radius]);
        put(&self.min);
        put(&self.max);
        d.extend_from_slice(&(self.items.len() as i32).to_le_bytes());
        d.extend_from_slice(&(self.step_count as i32).to_le_bytes());
        d.extend_from_slice(&self.frame_ms.to_le_bytes());
        for (i, it) in self.items.iter().enumerate() {
            if it.texture.len() > TEXTURE_LIMIT || it.faces.len() > COUNT_LIMIT || it.verts_per_step > COUNT_LIMIT {
                return Err(bad(format!("item {i}: the texture name, faces or vertices are too long")));
            }
            if it.verts_per_step.checked_mul(self.step_count) != Some(it.vertices.len()) {
                return Err(bad(format!("item {i}: {} vertices are not {} per step x {} steps", it.vertices.len(), it.verts_per_step, self.step_count)));
            }
            d.extend_from_slice(&(it.texture.len() as u32).to_le_bytes());
            d.extend_from_slice(&it.texture);
            d.extend_from_slice(&(it.faces.len() as i32).to_le_bytes());
            for f in &it.faces {
                for x in f {
                    d.extend_from_slice(&x.to_le_bytes());
                }
            }
            d.extend_from_slice(&(it.verts_per_step as i32).to_le_bytes());
            for v in &it.vertices {
                for c in v.pos.iter().chain(&v.normal) {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
                d.extend_from_slice(&v.diffuse.to_le_bytes());
                for c in v.uv {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
        }
        for c in self.box2.0.iter().chain(&self.box2.1) {
            d.extend_from_slice(&c.to_bits().to_le_bytes());
        }
        match &self.collision {
            None => d.extend_from_slice(&0i32.to_le_bytes()),
            Some(c) => {
                if c.points.len() > COUNT_LIMIT || c.faces.len() > COUNT_LIMIT {
                    return Err(bad("the collision mesh is too large".into()));
                }
                d.extend_from_slice(&1i32.to_le_bytes());
                d.extend_from_slice(&(c.points.len() as i32).to_le_bytes());
                for p in &c.points {
                    for x in p {
                        d.extend_from_slice(&x.to_bits().to_le_bytes());
                    }
                }
                d.extend_from_slice(&(c.faces.len() as i32).to_le_bytes());
                for f in &c.faces {
                    for x in f {
                        d.extend_from_slice(&x.to_le_bytes());
                    }
                }
            }
        }
        match Vani::parse(&d, "model") {
            Ok(_) => Ok(d),
            Err(e) => Err(bad(format!("the bytes do not read back: {e}"))),
        }
    }

    /// The texture name of an item: cp949 text up to the NUL (empty when the item names none or does not exist).
    pub fn texture_name(&self, item: usize) -> String {
        let raw = self.items.get(item).map(|i| i.texture.as_slice()).unwrap_or(&[]);
        decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[]))
    }

    /// The vertices of `item` at `step` (empty when either does not exist, or when `vertices` was shortened by the caller).
    pub fn step_vertices(&self, item: usize, step: usize) -> &[VaniVertex] {
        let Some(it) = self.items.get(item) else { return &[] };
        if step >= self.step_count {
            return &[];
        }
        it.vertices.get(step * it.verts_per_step..(step + 1) * it.verts_per_step).unwrap_or(&[])
    }

    /// (vertices per step, faces) summed over the items.
    pub fn counts(&self) -> (usize, usize) {
        (self.items.iter().map(|i| i.verts_per_step).sum(), self.items.iter().map(|i| i.faces.len()).sum())
    }

    /// Whether any vertex has a diffuse colour other than `0xFFFFFFFF`.
    pub fn has_vertex_colours(&self) -> bool {
        self.items.iter().any(|i| i.vertices.iter().any(|v| v.diffuse != 0xFFFF_FFFF))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::anim::{box_vani, vani_bytes, CollisionSpec, VaniItemSpec, VaniSpec, VaniVertSpec};

    fn vs(x: f32, diffuse: u32) -> VaniVertSpec {
        VaniVertSpec { pos: [x, 2.0, 3.0], normal: [0.0, 1.0, 0.0], diffuse, uv: [0.25, 0.5] }
    }

    fn spec() -> VaniSpec {
        let item = |t: &[u8], n: i32| VaniItemSpec {
            texture: t.to_vec(),
            faces: vec![[0, 1, 2]],
            vertices: (0..n * 3).map(|i| vs(i as f32, 0xFFFF_FFFF)).collect(),
        };
        VaniSpec {
            centre: [1.0, 2.0, 3.0],
            radius: 4.0,
            min: [0.0, 0.0, 0.0],
            max: [2.0, 4.0, 6.0],
            step_count: 2,
            frame_ms: 66,
            // each item holds verts_per_step x step_count vertices (3 per step here, 2 steps)
            items: vec![item(b"a.tga\0", 2), item(b"", 2)],
            box2: ([0.5; 3], [1.5; 3]),
            collision: None,
        }
    }

    fn parse(d: &[u8]) -> Result<Vani> {
        Vani::parse(d, "t.vani")
    }

    #[test]
    fn a_model_parses_and_writes_back() {
        for d in [box_vani(11, 33), vani_bytes(&spec())] {
            let m = parse(&d).unwrap();
            assert_eq!(m.to_bytes(), d);
        }
        let m = parse(&vani_bytes(&spec())).unwrap();
        assert_eq!((m.items.len(), m.step_count, m.frame_ms), (2, 2, 66));
        assert_eq!(m.counts(), (6, 2));
        assert_eq!((m.centre, m.radius), ([1.0, 2.0, 3.0], 4.0));
        assert_eq!(m.texture_name(0), "a.tga");
        assert_eq!(m.texture_name(1), "");
        assert_eq!(m.texture_name(9), "");
        assert_eq!(m.step_vertices(0, 1).len(), 3);
        assert_eq!(m.step_vertices(0, 1)[0].pos[0], 3.0);
        assert!(m.step_vertices(0, 2).is_empty() && m.step_vertices(5, 0).is_empty());
        // a caller who shortened the vertices gets an empty slice, not a panic
        let mut short = m.clone();
        short.items[0].vertices.truncate(2);
        assert!(short.step_vertices(0, 1).is_empty());
        assert!(m.collision.is_none() && !m.has_vertex_colours());
        let b = parse(&box_vani(11, 33)).unwrap();
        assert_eq!((b.step_count, b.frame_ms, b.items.len(), b.counts()), (11, 33, 1, (8, 12)));
    }

    #[test]
    fn truncations_and_an_extra_byte_are_errors() {
        for d in [box_vani(3, 66), vani_bytes(&spec())] {
            let mut longer = d.clone();
            longer.push(0);
            assert!(parse(&longer).is_err());
            for n in 0..d.len() {
                assert!(parse(&d[..n]).is_err(), "prefix of {n} of {} bytes", d.len());
            }
        }
    }

    #[test]
    fn a_vani_with_a_collision_mesh_round_trips() {
        // no file of US EP4.5 uses the collision branch: this is read from a synthetic file only
        let mut s = spec();
        s.collision = Some(CollisionSpec { points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], faces: vec![[0, 1, 2]] });
        let d = vani_bytes(&s);
        let m = parse(&d).unwrap();
        let c = m.collision.as_ref().unwrap();
        assert_eq!((c.points.len(), c.faces.len()), (3, 1));
        assert_eq!(m.to_bytes(), d);
        let mut longer = d.clone();
        longer.push(0);
        assert!(parse(&longer).is_err());
        for n in 0..d.len() {
            assert!(parse(&d[..n]).is_err(), "prefix of {n} of {} bytes", d.len());
        }
    }

    #[test]
    fn a_collision_flag_other_than_0_or_1_is_refused() {
        let d = vani_bytes(&spec());
        for flag in [2i32, 7, -1, i32::MAX] {
            let mut x = d.clone();
            let at = x.len() - 4;
            x[at..].copy_from_slice(&flag.to_le_bytes());
            let e = parse(&x).unwrap_err().to_string();
            assert!(e.contains("collision flag"), "{e}");
        }
    }

    #[test]
    fn colours_raw_floats_and_texture_bytes_survive() {
        let mut s = spec();
        s.items[0].texture = b"a.tga\0junk\xB0\xA1\0".to_vec();
        s.items[0].vertices[1].diffuse = 0x80FF_00AA;
        let d = vani_bytes(&s);
        let mut m = parse(&d).unwrap();
        assert!(m.has_vertex_colours());
        assert_eq!(m.items[0].vertices[1].diffuse, 0x80FF_00AA);
        assert_eq!(m.items[0].texture, b"a.tga\0junk\xB0\xA1\0".to_vec());
        assert_eq!(m.texture_name(0), "a.tga");
        m.radius = f32::from_bits(0xFFC0_0000);
        m.items[1].vertices[0].normal[1] = f32::from_bits(0x7FA0_0001);
        let out = m.to_bytes();
        let back = parse(&out).unwrap();
        assert_eq!(back.radius.to_bits(), 0xFFC0_0000);
        assert_eq!(back.items[1].vertices[0].normal[1].to_bits(), 0x7FA0_0001);
        assert_eq!(back.to_bytes(), out);
    }

    #[test]
    fn a_count_that_cannot_fit_is_refused_without_allocating() {
        let d = vani_bytes(&spec());
        // header: 4 x 4 (centre, radius) + 6 x 4 (box) = 40; item count at 40, step count at 44, frame_ms at 48
        for (at, value) in [(40usize, i32::MAX), (40, -1), (40, 5000), (44, i32::MAX), (44, -1), (44, 70_000)] {
            let mut x = d.clone();
            x[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "count {value} at {at}");
        }
        // first item: texture (4 + 6), face count (4) + 1 face (6), then vertices per step at 52 + 10 + 10
        let at = 52 + 10 + 10;
        for value in [i32::MAX, -1, 1 << 23] {
            let mut x = d.clone();
            x[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "vertices per step {value}");
        }
        // vertices per step x step count far larger than the bytes left: the bytes-left check refuses it
        let mut x = d.clone();
        x[44..48].copy_from_slice(&65_536i32.to_le_bytes());
        x[at..at + 4].copy_from_slice(&(1i32 << 24).to_le_bytes());
        assert!(parse(&x).is_err());
        assert!(parse(&[0xFF; 52]).is_err());
    }

    #[test]
    fn a_model_that_cannot_be_written_is_refused() {
        let mut m = parse(&vani_bytes(&spec())).unwrap();
        m.items[0].vertices.pop();
        assert!(m.try_to_bytes().is_err(), "vertices that are not per step x steps");
        let mut m = parse(&vani_bytes(&spec())).unwrap();
        m.step_count = STEP_LIMIT + 1;
        assert!(m.try_to_bytes().is_err());
        let mut m = parse(&vani_bytes(&spec())).unwrap();
        m.items = vec![m.items[0].clone(); ITEM_LIMIT + 1];
        assert!(m.try_to_bytes().is_err());
        assert_eq!(parse(&vani_bytes(&spec())).unwrap().try_to_bytes().unwrap(), vani_bytes(&spec()));
    }
}
