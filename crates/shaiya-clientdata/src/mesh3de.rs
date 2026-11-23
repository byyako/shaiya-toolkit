//! `.3DE` effect meshes (`Effect/3DE`, ...): a mesh with an optional block of frames. Layouts as observed in client files (little-endian,
//! no header; a file is read only when it ends exactly after its last field):
//!
//! ```text
//! standard:  i32 length + texture bytes (raw, NUL included)
//!            i32 vertex count; vertices (24 bytes each)
//!            i32 face count;   faces (3 x u16)
//!            i32 max_key                                    (inferred)
//!            i32 frame_count
//!            per frame: i32 key; then 20 bytes per vertex of the mesh (f32[3], f32 u, v)
//! static:    the standard layout cut after the faces (no frame block)
//! vertex (24 bytes):  f32 position[3]; i32 (-1 in every file read; kept); f32 u, v
//! ```
//!
//! Exactly one of the two layouts must read a file to the last byte; a file that none reads (or that both read) is refused. A file in
//! some other layout is never half-read: the error says that it does not follow the layout the toolkit reads. Floats are kept as stored and
//! the texture name as raw bytes (including the NUL and anything after it), which is what lets [`Mesh3de::to_bytes`] give back the bytes
//! [`Mesh3de::parse`] read. The meanings of `max_key` and of the 20-byte frame entries are inferred and were not checked in the game.

use crate::error::{decode_cp949, Cur, Error, Result};

/// One vertex, 24 bytes: position, a signed integer (kept as read), texture coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Vertex3de {
    pub pos: [f32; 3],
    pub flag: i32,
    pub uv: [f32; 2],
}

/// One frame entry per vertex, 20 bytes: a triple (inferred: a position change) and texture coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameVertex3de {
    pub delta: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame3de {
    pub key: i32,
    pub vertices: Vec<FrameVertex3de>,
}

/// The frame block of the standard layout.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameBlock3de {
    /// Inferred.
    pub max_key: i32,
    pub frames: Vec<Frame3de>,
}

/// Which layout a file was read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum De3Layout {
    /// With the frame block (the block may hold 0 frames).
    Standard,
    /// Ends after the faces.
    Static,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh3de {
    /// The texture name as stored: raw bytes, the NUL and anything after it included (may be empty).
    pub texture: Vec<u8>,
    pub vertices: Vec<Vertex3de>,
    pub faces: Vec<[u16; 3]>,
    /// `None` for the static layout.
    pub block: Option<FrameBlock3de>,
}

const VERTEX: usize = 24;
const FRAME_VERTEX: usize = 20;
const FACE: usize = 6;
pub(crate) const COUNT_LIMIT: usize = 1 << 24;
pub(crate) const FRAME_LIMIT: usize = 1 << 16;
pub(crate) const TEXTURE_LIMIT: usize = 1 << 20;

/// The refusal text for a file that neither layout reads.
pub const NOT_THE_LAYOUT: &str = "does not follow the .3DE layout the toolkit reads";

fn read(data: &[u8], file: &str, with_block: bool) -> Result<Mesh3de> {
    let mut r = Cur::new(data, file);
    let texture = r.raw_str_i32("texture name")?.to_vec();
    let nv = r.count(VERTEX, COUNT_LIMIT, "vertex count")?;
    let mut vertices = Vec::with_capacity(nv);
    for _ in 0..nv {
        let pos = r.f3("vertex position")?;
        let flag = r.i32("vertex field")?;
        let uv = [r.f32("vertex u")?, r.f32("vertex v")?];
        vertices.push(Vertex3de { pos, flag, uv });
    }
    let nf = r.count(FACE, COUNT_LIMIT, "face count")?;
    let mut faces = Vec::with_capacity(nf);
    for _ in 0..nf {
        faces.push([r.u16("face index")?, r.u16("face index")?, r.u16("face index")?]);
    }
    let mut block = None;
    if with_block {
        let max_key = r.i32("max_key")?;
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
                vs.push(FrameVertex3de { delta: r.f3("frame entry")?, uv: [r.f32("frame u")?, r.f32("frame v")?] });
            }
            frames.push(Frame3de { key, vertices: vs });
        }
        block = Some(FrameBlock3de { max_key, frames });
    }
    r.end("mesh")?;
    Ok(Mesh3de { texture, vertices, faces, block })
}

/// The outcome of choosing among the layouts that read a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    One(usize),
    None,
    Ambiguous,
}

/// `ok[i]` is true when layout `i` (standard, static) reads the file to the last byte: exactly one is accepted.
pub(crate) fn pick_layout(ok: &[bool; 2]) -> Pick {
    match ok.iter().filter(|b| **b).count() {
        0 => Pick::None,
        1 => Pick::One(ok.iter().position(|b| *b).unwrap_or(0)),
        _ => Pick::Ambiguous,
    }
}

impl Mesh3de {
    /// Reads a whole `.3DE` with each layout; exactly one must read it to the last byte. A file that none reads is refused with
    /// [`NOT_THE_LAYOUT`] (the standard attempt's error follows in the text); a file that both read is refused as ambiguous.
    pub fn parse(data: &[u8], file: &str) -> Result<Mesh3de> {
        if data.is_empty() {
            return Err(Error::Format { file: file.to_string(), what: "empty file".into(), offset: 0 });
        }
        let attempts = [read(data, file, true), read(data, file, false)];
        let ok = [attempts[0].is_ok(), attempts[1].is_ok()];
        match pick_layout(&ok) {
            Pick::One(i) => attempts.into_iter().nth(i).and_then(|a| a.ok()).ok_or_else(|| Error::Format { file: file.to_string(), what: "unreadable".into(), offset: 0 }),
            Pick::Ambiguous => Err(Error::Format { file: file.to_string(), what: "ambiguous layout: both layouts read this file to the last byte".into(), offset: 0 }),
            Pick::None => {
                let why = attempts.into_iter().next().and_then(|a| a.err()).map(|e| e.to_string()).unwrap_or_default();
                Err(Error::Format { file: file.to_string(), what: format!("{NOT_THE_LAYOUT} ({why})"), offset: 0 })
            }
        }
    }

    pub fn layout(&self) -> De3Layout {
        if self.block.is_some() {
            De3Layout::Standard
        } else {
            De3Layout::Static
        }
    }

    /// The bytes of this mesh; for a parsed file, exactly the bytes that were read.
    ///
    /// Panics when the mesh cannot be written (see [`Mesh3de::try_to_bytes`]), which no parsed file can hold.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("mesh cannot be written")
    }

    /// Like [`Mesh3de::to_bytes`], but refuses a mesh that does not fit the file: a frame that does not hold one entry per vertex, a
    /// count above what the reader accepts. The bytes are read back with [`Mesh3de::parse`] and refused unless they read with the same
    /// layout, so a mesh that the other layout also reads is an error, never bytes read differently.
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let bad = |what: String| Error::Format { file: "mesh".into(), what, offset: 0 };
        if self.texture.len() > TEXTURE_LIMIT || self.vertices.len() > COUNT_LIMIT || self.faces.len() > COUNT_LIMIT {
            return Err(bad("the texture name, vertices or faces are too many".into()));
        }
        if let Some(b) = &self.block {
            if b.frames.len() > FRAME_LIMIT {
                return Err(bad("too many frames".into()));
            }
            if let Some(i) = b.frames.iter().position(|f| f.vertices.len() != self.vertices.len()) {
                return Err(bad(format!("frame {i}: {} entries for {} vertices", b.frames[i].vertices.len(), self.vertices.len())));
            }
        }
        let mut d = Vec::new();
        d.extend_from_slice(&(self.texture.len() as u32).to_le_bytes());
        d.extend_from_slice(&self.texture);
        d.extend_from_slice(&(self.vertices.len() as i32).to_le_bytes());
        for v in &self.vertices {
            for c in v.pos {
                d.extend_from_slice(&c.to_bits().to_le_bytes());
            }
            d.extend_from_slice(&v.flag.to_le_bytes());
            for c in v.uv {
                d.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
        d.extend_from_slice(&(self.faces.len() as i32).to_le_bytes());
        for f in &self.faces {
            for i in f {
                d.extend_from_slice(&i.to_le_bytes());
            }
        }
        if let Some(b) = &self.block {
            d.extend_from_slice(&b.max_key.to_le_bytes());
            d.extend_from_slice(&(b.frames.len() as i32).to_le_bytes());
            for f in &b.frames {
                d.extend_from_slice(&f.key.to_le_bytes());
                for v in &f.vertices {
                    for c in v.delta.iter().chain(&v.uv) {
                        d.extend_from_slice(&c.to_bits().to_le_bytes());
                    }
                }
            }
        }
        match Mesh3de::parse(&d, "mesh") {
            Ok(back) if back.layout() == self.layout() => Ok(d),
            Ok(back) => Err(bad(format!("the bytes read back as {:?}, not {:?}", back.layout(), self.layout()))),
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

    /// The number of frames (0 for the static layout and for a standard file whose block holds none).
    pub fn frame_count(&self) -> usize {
        self.block.as_ref().map_or(0, |b| b.frames.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::anim::{box_3de, mesh3de_bytes, Block3deSpec, Mesh3deSpec, Vert3deSpec};

    fn parse(d: &[u8]) -> Result<Mesh3de> {
        Mesh3de::parse(d, "t.3de")
    }

    #[test]
    fn both_layouts_parse_and_write_back() {
        for (d, layout, frames) in [(box_3de(Some(4)), De3Layout::Standard, 4), (box_3de(Some(0)), De3Layout::Standard, 0), (box_3de(None), De3Layout::Static, 0)] {
            let m = parse(&d).unwrap();
            assert_eq!((m.layout(), m.frame_count(), m.counts()), (layout, frames, (8, 12)));
            assert_eq!(m.to_bytes(), d);
        }
        let m = parse(&box_3de(Some(4))).unwrap();
        assert_eq!(m.texture_name(), "fx.tga");
        assert_eq!(m.block.as_ref().map(|b| b.max_key), Some(3));
        assert_eq!(m.vertices[0].flag, -1);
        assert!(parse(&box_3de(None)).unwrap().block.is_none());
    }

    #[test]
    fn a_3de_in_the_static_layout_is_read_and_a_cut_standard_file_is_the_static_one() {
        // a standard file cut right after its faces is a complete static file; every other cut is an error
        let d = box_3de(Some(3));
        let static_len = box_3de(None).len();
        for n in 0..d.len() {
            assert_eq!(parse(&d[..n]).is_ok(), n == static_len, "prefix of {n} of {} bytes", d.len());
        }
        let mut longer = d.clone();
        longer.push(0);
        assert!(parse(&longer).is_err());
        let mut longer = box_3de(None);
        longer.push(0);
        assert!(parse(&longer).is_err());
    }

    #[test]
    fn a_two_layout_file_is_refused_as_ambiguous() {
        // the decision function: exactly one layout may read a file
        assert_eq!(pick_layout(&[true, false]), Pick::One(0));
        assert_eq!(pick_layout(&[false, true]), Pick::One(1));
        assert_eq!(pick_layout(&[false, false]), Pick::None);
        assert_eq!(pick_layout(&[true, true]), Pick::Ambiguous);
        // the two layouts cannot both read a real file: the standard one needs 8 more bytes than the static one ends with
        for d in [box_3de(None), box_3de(Some(0)), box_3de(Some(2))] {
            assert_eq!([read(&d, "t", true).is_ok(), read(&d, "t", false).is_ok()].iter().filter(|b| **b).count(), 1);
        }
    }

    #[test]
    fn a_3de_in_another_layout_is_refused_with_the_reason() {
        // a frame block that is cut, an odd tail after the faces, a file that is only a texture name, bytes that are not a mesh
        let mut cut = box_3de(Some(2));
        let end = cut.len();
        cut.truncate(end - 5);
        let mut tail = box_3de(None);
        tail.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        for d in [cut, tail, vec![0, 0, 0, 0], vec![7u8; 30]] {
            let e = parse(&d).unwrap_err().to_string();
            assert!(e.contains(NOT_THE_LAYOUT), "{e}");
        }
    }

    #[test]
    fn an_empty_3de_is_empty() {
        let e = parse(&[]).unwrap_err().to_string();
        assert!(e.contains("empty file"), "{e}");
    }

    #[test]
    fn floats_texture_bytes_and_the_vertex_field_survive() {
        let mut m = parse(&box_3de(Some(2))).unwrap();
        m.texture = b"a.tga\0junk\xB0\xA1\0".to_vec();
        m.vertices[0].flag = 12345;
        m.vertices[1].pos[2] = f32::from_bits(0xFFC0_0000);
        m.block.as_mut().unwrap().frames[1].vertices[3].delta[0] = f32::from_bits(0x7FA0_0001);
        m.block.as_mut().unwrap().max_key = -5;
        let d = m.to_bytes();
        let back = parse(&d).unwrap();
        assert_eq!(back.texture_name(), "a.tga");
        assert_eq!((back.vertices[0].flag, back.vertices[1].pos[2].to_bits()), (12345, 0xFFC0_0000));
        assert_eq!(back.block.as_ref().map(|b| (b.max_key, b.frames[1].vertices[3].delta[0].to_bits())), Some((-5, 0x7FA0_0001)));
        assert_eq!(back.to_bytes(), d);
    }

    #[test]
    fn a_count_that_cannot_fit_is_refused_without_allocating() {
        let d = box_3de(Some(2));
        // texture: 4 + 7 bytes ("fx.tga" and its NUL), so the vertex count is at 11
        for value in [i32::MAX, -1, 1 << 23] {
            let mut x = d.clone();
            x[11..15].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "vertex count {value}");
        }
        // the frame count: after the vertices, the faces and max_key
        let at = 4 + 7 + 4 + 8 * 24 + 4 + 12 * 6 + 4;
        for value in [i32::MAX, -1, 65_536, 3] {
            let mut x = d.clone();
            x[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(&x).is_err(), "frame count {value}");
        }
        assert!(parse(&[0xFF, 0xFF, 0xFF, 0x7F]).is_err());
    }

    #[test]
    fn a_mesh_that_cannot_be_written_is_refused() {
        let mut m = parse(&box_3de(Some(2))).unwrap();
        m.block.as_mut().unwrap().frames[0].vertices.pop();
        assert!(m.try_to_bytes().is_err(), "a frame without one entry per vertex");
        let mut m = parse(&box_3de(Some(2))).unwrap();
        let f = m.block.as_ref().unwrap().frames[0].clone();
        m.block.as_mut().unwrap().frames = vec![f; FRAME_LIMIT + 1];
        assert!(m.try_to_bytes().is_err());
        assert_eq!(parse(&box_3de(Some(2))).unwrap().try_to_bytes().unwrap(), box_3de(Some(2)));
    }

    #[test]
    fn a_standard_mesh_with_no_vertices_reads_as_it_was_built() {
        let d = mesh3de_bytes(&Mesh3deSpec { texture: Vec::new(), vertices: Vec::<Vert3deSpec>::new(), faces: Vec::new(), block: Some(Block3deSpec { max_key: 0, frames: vec![(0, Vec::new())] }) });
        let m = parse(&d).unwrap();
        assert_eq!((m.vertices.len(), m.frame_count()), (0, 1));
        assert_eq!(m.to_bytes(), d);
    }
}
