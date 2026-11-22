//! `.3DO` meshes (`Item/3DO`, ...): a mesh with optional vertex frames. Layout as observed in client files (little-endian, no header;
//! a file is read only when it ends exactly after the last frame):
//!
//! ```text
//! i32 length + texture bytes (raw, NUL included; usually empty)
//! i32 vertex count; vertices (32 bytes each)
//! i32 face count;   faces (3 x u16)
//! i32 frame_rate                              (inferred: frames per second)
//! i32 frame_count
//! per frame: i32 key; then 24 bytes per vertex of the mesh (position[3], normal[3]: the whole vertex set of that frame)
//! vertex (32 bytes):  f32 position[3]; f32 normal[3]; f32 u, v
//! ```
//!
//! Floats are kept as stored and the texture name as raw bytes (including the NUL and anything after it), so [`Mesh3do::to_bytes`] gives back
//! the bytes [`Mesh3do::parse`] read. The meaning of `frame_rate` is inferred and was not checked in the game.

use crate::error::{decode_cp949, Cur, Error, Result};

/// One vertex, 32 bytes: position, normal, texture coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Vertex3do {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// One frame: a key and the position and normal of every vertex of the mesh (24 bytes each in the file).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame3do {
    pub key: i32,
    pub vertices: Vec<([f32; 3], [f32; 3])>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh3do {
    /// The texture name as stored: raw bytes, the NUL and anything after it included (usually empty).
    pub texture: Vec<u8>,
    pub vertices: Vec<Vertex3do>,
    pub faces: Vec<[u16; 3]>,
    /// Inferred: frames per second.
    pub frame_rate: i32,
    pub frames: Vec<Frame3do>,
}

const VERTEX: usize = 32;
const FRAME_VERTEX: usize = 24;
const FACE: usize = 6;
const COUNT_LIMIT: usize = 1 << 24;
const FRAME_LIMIT: usize = 1 << 16;
const TEXTURE_LIMIT: usize = 1 << 20;

impl Mesh3do {
    /// Reads a whole `.3DO`; the file must end exactly after the last frame.
    pub fn parse(data: &[u8], file: &str) -> Result<Mesh3do> {
        let mut r = Cur::new(data, file);
        let texture = r.raw_str_i32("texture name")?.to_vec();
        let nv = r.count(VERTEX, COUNT_LIMIT, "vertex count")?;
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            let pos = r.f3("vertex position")?;
            let normal = r.f3("vertex normal")?;
            let uv = [r.f32("vertex u")?, r.f32("vertex v")?];
            vertices.push(Vertex3do { pos, normal, uv });
        }
        let nf = r.count(FACE, COUNT_LIMIT, "face count")?;
        let mut faces = Vec::with_capacity(nf);
        for _ in 0..nf {
            faces.push([r.u16("face index")?, r.u16("face index")?, r.u16("face index")?]);
        }
        let frame_rate = r.i32("frame_rate")?;
        // a frame is at least its key
        let n_frames = r.count(4, FRAME_LIMIT, "frame count")?;
        let frame_bytes = nv * FRAME_VERTEX + 4;
        if !n_frames.checked_mul(frame_bytes).is_some_and(|b| b <= r.left()) {
            return Err(r.fail(format!("frames: {n_frames} frames of {nv} vertices do not fit in the file")));
        }
        let mut frames = Vec::with_capacity(n_frames);
        for _ in 0..n_frames {
            let key = r.i32("frame key")?;
            let mut vs = Vec::with_capacity(nv);
            for _ in 0..nv {
                vs.push((r.f3("frame position")?, r.f3("frame normal")?));
            }
            frames.push(Frame3do { key, vertices: vs });
        }
        r.end("mesh")?;
        Ok(Mesh3do { texture, vertices, faces, frame_rate, frames })
    }

    /// The bytes of this mesh; for a parsed file, exactly the bytes that were read.
    ///
    /// Panics when the mesh cannot be written (see [`Mesh3do::try_to_bytes`]), which no parsed file can hold.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("mesh cannot be written")
    }

    /// Like [`Mesh3do::to_bytes`], but refuses a mesh that does not fit the file: a frame that does not hold one entry per vertex, a count
    /// above what the reader accepts. The bytes are read back with [`Mesh3do::parse`] and refused unless they read.
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let bad = |what: String| Error::Format { file: "mesh".into(), what, offset: 0 };
        if self.texture.len() > TEXTURE_LIMIT || self.vertices.len() > COUNT_LIMIT || self.faces.len() > COUNT_LIMIT || self.frames.len() > FRAME_LIMIT {
            return Err(bad("the texture name, vertices, faces or frames are too many".into()));
        }
        if let Some(i) = self.frames.iter().position(|f| f.vertices.len() != self.vertices.len()) {
            return Err(bad(format!("frame {i}: {} entries for {} vertices", self.frames[i].vertices.len(), self.vertices.len())));
        }
        let mut d = Vec::new();
        d.extend_from_slice(&(self.texture.len() as u32).to_le_bytes());
        d.extend_from_slice(&self.texture);
        d.extend_from_slice(&(self.vertices.len() as i32).to_le_bytes());
        for v in &self.vertices {
            for c in v.pos.iter().chain(&v.normal).chain(&v.uv) {
                d.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
        d.extend_from_slice(&(self.faces.len() as i32).to_le_bytes());
        for f in &self.faces {
            for i in f {
                d.extend_from_slice(&i.to_le_bytes());
            }
        }
        d.extend_from_slice(&self.frame_rate.to_le_bytes());
        d.extend_from_slice(&(self.frames.len() as i32).to_le_bytes());
        for f in &self.frames {
            d.extend_from_slice(&f.key.to_le_bytes());
            for (p, n) in &f.vertices {
                for c in p.iter().chain(n) {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
        }
        match Mesh3do::parse(&d, "mesh") {
            Ok(_) => Ok(d),
            Err(e) => Err(bad(format!("the bytes do not read back: {e}"))),
        }
    }

    /// The texture name: cp949 text up to the NUL (empty when the mesh names none).
    pub fn texture_name(&self) -> String {
        decode_cp949(self.texture.split(|&b| b == 0).next().unwrap_or(&[]))
    }

    /// (vertices, faces).
    pub fn counts(&self) -> (usize, usize) {
        (self.vertices.len(), self.faces.len())
    }

    /// The vertices of the mesh with the positions and normals of frame `frame` (empty when the frame does not exist); the texture
    /// coordinates stay those of the rest mesh.
    pub fn frame_vertices(&self, frame: usize) -> Vec<Vertex3do> {
        let Some(f) = self.frames.get(frame) else { return Vec::new() };
        self.vertices.iter().zip(&f.vertices).map(|(v, (pos, normal))| Vertex3do { pos: *pos, normal: *normal, uv: v.uv }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::anim::{animated_3do, box_3do, mesh3do_bytes, Mesh3doSpec, Vert3doSpec};

    fn parse(d: &[u8]) -> Result<Mesh3do> {
        Mesh3do::parse(d, "t.3do")
    }

    #[test]
    fn a_mesh_with_and_without_frames_parses_and_writes_back() {
        for d in [box_3do(), animated_3do(11, 20), animated_3do(1, 5)] {
            let m = parse(&d).unwrap();
            assert_eq!(m.to_bytes(), d);
        }
        let m = parse(&box_3do()).unwrap();
        assert_eq!((m.counts(), m.frames.len(), m.frame_rate, m.texture_name().as_str()), ((8, 12), 0, 0, ""));
        let m = parse(&animated_3do(11, 20)).unwrap();
        assert_eq!((m.counts(), m.frames.len(), m.frame_rate, m.texture_name().as_str()), ((8, 12), 11, 20, "box.tga"));
        assert_eq!((m.frames[0].key, m.frames[10].key), (0, 10));
    }

    #[test]
    fn an_animated_3do_exposes_its_frames() {
        // the frames of an item mesh with frames: positions replaced per frame, the texture coordinates stay those of the rest mesh
        let m = parse(&animated_3do(11, 20)).unwrap();
        let rest = &m.vertices;
        let (f0, f10) = (m.frame_vertices(0), m.frame_vertices(10));
        assert_eq!((f0.len(), f10.len()), (8, 8));
        let top = rest.iter().position(|v| v.pos[1] > 0.5).unwrap();
        assert_eq!(f0[top].pos, rest[top].pos);
        assert_eq!(f10[top].pos[1], rest[top].pos[1] + 1.0);
        assert_eq!(f10[top].uv, rest[top].uv);
        assert!(m.frame_vertices(11).is_empty());
        assert!(parse(&box_3do()).unwrap().frame_vertices(0).is_empty());
    }

    #[test]
    fn truncations_and_an_extra_byte_are_errors() {
        for d in [box_3do(), animated_3do(3, 20)] {
            let mut longer = d.clone();
            longer.push(0);
            assert!(parse(&longer).is_err());
            for n in 0..d.len() {
                assert!(parse(&d[..n]).is_err(), "prefix of {n} of {} bytes", d.len());
            }
        }
    }

    #[test]
    fn floats_texture_bytes_and_the_rate_survive() {
        let mut m = parse(&animated_3do(2, 20)).unwrap();
        m.texture = b"a.tga\0junk\xB0\xA1\0".to_vec();
        m.frame_rate = -7;
        m.vertices[0].normal[1] = f32::from_bits(0xFFC0_0000);
        m.frames[1].vertices[2].0[0] = f32::from_bits(0x7FA0_0001);
        let d = m.to_bytes();
        let back = parse(&d).unwrap();
        assert_eq!(back.texture_name(), "a.tga");
        assert_eq!((back.frame_rate, back.vertices[0].normal[1].to_bits(), back.frames[1].vertices[2].0[0].to_bits()), (-7, 0xFFC0_0000, 0x7FA0_0001));
        assert_eq!(back.to_bytes(), d);
    }

    #[test]
    fn a_count_that_cannot_fit_is_refused_without_allocating() {
        let d = box_3do();
        // texture length 0 at 0, vertex count at 4
        for value in [i32::MAX, -1, 1 << 23] {
            let mut x = d.clone();
            x[4..8].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "vertex count {value}");
        }
        // face count after the 8 vertices
        let at = 8 + 8 * 32;
        for value in [i32::MAX, -1] {
            let mut x = d.clone();
            x[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "face count {value}");
        }
        // the frame count is the last 4 bytes
        for value in [i32::MAX, -1, 1, 65_536] {
            let mut x = d.clone();
            let end = x.len();
            x[end - 4..].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "frame count {value}");
        }
        assert!(parse(&[0xFF, 0xFF, 0xFF, 0x7F]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn a_mesh_that_cannot_be_written_is_refused() {
        let mut m = parse(&animated_3do(2, 20)).unwrap();
        m.frames[0].vertices.pop();
        assert!(m.try_to_bytes().is_err(), "a frame without one entry per vertex");
        let mut m = parse(&animated_3do(2, 20)).unwrap();
        m.frames = vec![m.frames[0].clone(); FRAME_LIMIT + 1];
        assert!(m.try_to_bytes().is_err());
        assert_eq!(parse(&animated_3do(2, 20)).unwrap().try_to_bytes().unwrap(), animated_3do(2, 20));
    }

    #[test]
    fn a_mesh_with_no_vertices_and_frames_is_read() {
        let d = mesh3do_bytes(&Mesh3doSpec { texture: Vec::new(), vertices: Vec::<Vert3doSpec>::new(), faces: Vec::new(), frame_rate: 20, frames: vec![(0, Vec::new()), (1, Vec::new())] });
        let m = parse(&d).unwrap();
        assert_eq!((m.vertices.len(), m.frames.len()), (0, 2));
        assert_eq!(m.to_bytes(), d);
    }
}
