//! `.3DC` skinned meshes (`Character/<race>/3DC`, `Monster/3DC`, ...): the mesh a `.ANI` skeleton moves. Layouts as observed in client
//! files (all little-endian, no header; a file is read only when it ends exactly after the last face):
//!
//! ```text
//! skinned:            i32 length + texture bytes (raw, NUL included)
//!                     i32 bone count; f32 matrix[16] x bone count        (the mesh's own inverse bind pose)
//!                     i32 vertex count; vertices (40 bytes each)
//!                     i32 face count;   faces (3 x u16)
//! skinned + second:   two skinned meshes back to back
//! cloak:              the skinned layout without the bone block
//! vertex (40 bytes):  f32 x, y, z; f32 weight; u8 bone1; u8 bone2; u16 pad; f32 normal[3]; f32 u, v
//! ```
//!
//! The matrices are the mesh's own inverse bind pose; they are not the inverse of an animation's reference matrices. Floats are kept as
//! stored and the texture name as raw bytes (including the NUL and anything after it), which is what lets [`C3dc::to_bytes`] give back the
//! bytes [`C3dc::parse`] read. The `pad` field is not always 0 and is kept. A file that two layouts both read to the last byte is refused.

use crate::error::{decode_cp949, Cur, Error, Result};

/// Which of the three layouts a file was read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum C3dcLayout {
    Skinned,
    SkinnedWithSecondMesh,
    /// No bone block.
    Cloak,
}

/// One vertex, 40 bytes: position, weight of `bone1` (the rest goes to `bone2`), the two bone indices, the `pad` field, normal, texture
/// coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct C3dcVertex {
    pub pos: [f32; 3],
    pub weight: f32,
    pub bone1: u8,
    pub bone2: u8,
    pub pad: u16,
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct C3dcMesh {
    /// The texture name as stored: raw bytes, the NUL and anything after it included.
    pub texture: Vec<u8>,
    /// The mesh's own matrices, row-major, one per bone; empty for a cloak.
    pub bones: Vec<[f32; 16]>,
    pub vertices: Vec<C3dcVertex>,
    pub faces: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct C3dc {
    pub layout: C3dcLayout,
    /// One mesh, or two for [`C3dcLayout::SkinnedWithSecondMesh`].
    pub meshes: Vec<C3dcMesh>,
}

const VERTEX: usize = 40;
const FACE: usize = 6;
const BONE_LIMIT: usize = 4096;
const COUNT_LIMIT: usize = 1 << 24;
const TEXTURE_LIMIT: usize = 1 << 20;

fn read_mesh(r: &mut Cur, with_bones: bool) -> Result<C3dcMesh> {
    let texture = r.raw_str_i32("texture name")?.to_vec();
    let mut bones = Vec::new();
    if with_bones {
        let n = r.count(64, BONE_LIMIT, "bone count")?;
        bones.reserve(n);
        for _ in 0..n {
            let mut m = [0f32; 16];
            for c in m.iter_mut() {
                *c = r.f32("bone matrix")?;
            }
            bones.push(m);
        }
    }
    let nv = r.count(VERTEX, COUNT_LIMIT, "vertex count")?;
    let mut vertices = Vec::with_capacity(nv);
    for _ in 0..nv {
        let pos = r.f3("vertex position")?;
        let weight = r.f32("vertex weight")?;
        let bone1 = r.u8("vertex bone")?;
        let bone2 = r.u8("vertex bone")?;
        let pad = r.u16("vertex pad")?;
        let normal = r.f3("vertex normal")?;
        let uv = [r.f32("vertex u")?, r.f32("vertex v")?];
        vertices.push(C3dcVertex { pos, weight, bone1, bone2, pad, normal, uv });
    }
    let nf = r.count(FACE, COUNT_LIMIT, "face count")?;
    let mut faces = Vec::with_capacity(nf);
    for _ in 0..nf {
        faces.push([r.u16("face index")?, r.u16("face index")?, r.u16("face index")?]);
    }
    Ok(C3dcMesh { texture, bones, vertices, faces })
}

/// The outcome of choosing among the layouts that read a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    One(usize),
    None,
    Ambiguous,
}

/// `ok[i]` is true when layout `i` (skinned, skinned + second mesh, cloak) reads the file to the last byte: exactly one is accepted.
pub(crate) fn pick_layout(ok: &[bool; 3]) -> Pick {
    match ok.iter().filter(|b| **b).count() {
        0 => Pick::None,
        1 => Pick::One(ok.iter().position(|b| *b).unwrap_or(0)),
        _ => Pick::Ambiguous,
    }
}

fn whole(data: &[u8], file: &str, meshes: usize, with_bones: bool) -> Result<Vec<C3dcMesh>> {
    let mut r = Cur::new(data, file);
    let mut out = Vec::with_capacity(meshes);
    for _ in 0..meshes {
        out.push(read_mesh(&mut r, with_bones)?);
    }
    r.end("mesh")?;
    Ok(out)
}

impl C3dc {
    /// Reads a whole `.3DC` with each layout; exactly one must read it to the last byte. When none does, the error is the skinned
    /// attempt's; when several do, the file is refused as ambiguous.
    pub fn parse(data: &[u8], file: &str) -> Result<C3dc> {
        let attempts = [whole(data, file, 1, true), whole(data, file, 2, true), whole(data, file, 1, false)];
        let ok = [attempts[0].is_ok(), attempts[1].is_ok(), attempts[2].is_ok()];
        match pick_layout(&ok) {
            Pick::One(i) => {
                let layout = [C3dcLayout::Skinned, C3dcLayout::SkinnedWithSecondMesh, C3dcLayout::Cloak][i];
                let meshes = attempts.into_iter().nth(i).and_then(|a| a.ok()).unwrap_or_default();
                Ok(C3dc { layout, meshes })
            }
            Pick::Ambiguous => Err(Error::Format { file: file.to_string(), what: "ambiguous layout: more than one layout reads this file to the last byte".into(), offset: 0 }),
            Pick::None => Err(attempts.into_iter().next().and_then(|a| a.err()).unwrap_or_else(|| Error::Format { file: file.to_string(), what: "unreadable".into(), offset: 0 })),
        }
    }

    /// The bytes of this mesh; for a parsed file, exactly the bytes that were read.
    ///
    /// Panics when the mesh cannot be written (see [`C3dc::try_to_bytes`]), which no parsed file can hold.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("mesh cannot be written")
    }

    /// Like [`C3dc::to_bytes`], but refuses a mesh that does not fit the file (a layout that does not match the meshes, a count above
    /// what the reader accepts). The bytes are read back with [`C3dc::parse`] and refused unless they read with the same layout, so a
    /// mesh that another layout also reads (or that the reader refuses) is an error, never bytes read differently.
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let bad = |what: String| Error::Format { file: "mesh".into(), what, offset: 0 };
        let (count, with_bones) = match self.layout {
            C3dcLayout::Skinned => (1, true),
            C3dcLayout::SkinnedWithSecondMesh => (2, true),
            C3dcLayout::Cloak => (1, false),
        };
        if self.meshes.len() != count {
            return Err(bad(format!("{:?} needs {count} mesh(es), found {}", self.layout, self.meshes.len())));
        }
        let mut d = Vec::new();
        for (i, m) in self.meshes.iter().enumerate() {
            if m.texture.len() > TEXTURE_LIMIT {
                return Err(bad(format!("mesh {i}: the texture name is too long")));
            }
            if !with_bones && !m.bones.is_empty() {
                return Err(bad(format!("mesh {i}: a cloak has no bone block")));
            }
            if m.bones.len() > BONE_LIMIT || m.vertices.len() > COUNT_LIMIT || m.faces.len() > COUNT_LIMIT {
                return Err(bad(format!("mesh {i}: too many bones, vertices or faces")));
            }
            d.extend_from_slice(&(m.texture.len() as u32).to_le_bytes());
            d.extend_from_slice(&m.texture);
            if with_bones {
                d.extend_from_slice(&(m.bones.len() as i32).to_le_bytes());
                for b in &m.bones {
                    for c in b {
                        d.extend_from_slice(&c.to_bits().to_le_bytes());
                    }
                }
            }
            d.extend_from_slice(&(m.vertices.len() as i32).to_le_bytes());
            for v in &m.vertices {
                for c in v.pos {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
                d.extend_from_slice(&v.weight.to_bits().to_le_bytes());
                d.push(v.bone1);
                d.push(v.bone2);
                d.extend_from_slice(&v.pad.to_le_bytes());
                for c in v.normal {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
                for c in v.uv {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
            d.extend_from_slice(&(m.faces.len() as i32).to_le_bytes());
            for f in &m.faces {
                for i in f {
                    d.extend_from_slice(&i.to_le_bytes());
                }
            }
        }
        match C3dc::parse(&d, "mesh") {
            Ok(back) if back.layout == self.layout => Ok(d),
            Ok(back) => Err(bad(format!("the bytes read back as {:?}, not {:?}", back.layout, self.layout))),
            Err(e) => Err(bad(format!("the bytes do not read back: {e}"))),
        }
    }

    /// The first mesh's texture name: cp949 text up to the NUL.
    pub fn texture_name(&self) -> String {
        let raw = self.meshes.first().map(|m| m.texture.as_slice()).unwrap_or(&[]);
        decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[]))
    }

    /// The highest bone index any vertex of the first mesh names (either of its two bones), if it has vertices.
    pub fn max_vertex_bone(&self) -> Option<u8> {
        self.meshes.first()?.vertices.iter().map(|v| v.bone1.max(v.bone2)).max()
    }

    /// (vertices, faces) over all meshes of the file.
    pub fn counts(&self) -> (usize, usize) {
        (self.meshes.iter().map(|m| m.vertices.len()).sum(), self.meshes.iter().map(|m| m.faces.len()).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::anim::{arm_mesh_3dc, c3dc_bytes, c3dc_cloak_bytes, identity_bits, VertSpec};

    fn vert(bone1: u8, bone2: u8, pad: u16) -> VertSpec {
        VertSpec { pos: [1.0, 2.0, 3.0], weight: 0.25, bone1, bone2, pad, normal: [0.0, 1.0, 0.0], uv: [0.5, 0.75] }
    }

    fn skinned() -> Vec<u8> {
        // matrices with no zero bytes, so no cut of the file happens to read as another layout
        c3dc_bytes(b"skin.dds\0", &[[0x3F80_0001; 16], [0x3F80_0002; 16]],&[vert(0, 1, 0), vert(1, 0, 0), vert(1, 1, 0)], &[[0, 1, 2]])
    }

    fn two() -> Vec<u8> {
        let mut d = skinned();
        d.extend(c3dc_bytes(b"b.dds\0", &[identity_bits()], &[vert(0, 0, 0)], &[]));
        d
    }

    fn cloak() -> Vec<u8> {
        c3dc_cloak_bytes(b"cape.dds\0", &[vert(0, 0, 0), vert(0, 0, 0), vert(0, 0, 0)], &[[2, 1, 0]])
    }

    fn parse(d: &[u8]) -> Result<C3dc> {
        C3dc::parse(d, "t.3dc")
    }

    #[test]
    fn each_layout_parses_and_writes_back() {
        for (d, layout, meshes, counts) in [
            (skinned(), C3dcLayout::Skinned, 1, (3, 1)),
            (two(), C3dcLayout::SkinnedWithSecondMesh, 2, (4, 1)),
            (cloak(), C3dcLayout::Cloak, 1, (3, 1)),
            (arm_mesh_3dc(), C3dcLayout::Skinned, 1, (16, 24)),
        ] {
            let m = parse(&d).unwrap();
            assert_eq!((m.layout, m.meshes.len(), m.counts()), (layout, meshes, counts));
            assert_eq!(m.to_bytes(), d);
        }
        let m = parse(&skinned()).unwrap();
        assert_eq!(m.texture_name(), "skin.dds");
        assert_eq!(m.meshes[0].bones.len(), 2);
        assert_eq!(m.max_vertex_bone(), Some(1));
        assert_eq!(m.meshes[0].vertices[0].uv, [0.5, 0.75]);
        assert_eq!(m.meshes[0].faces, vec![[0, 1, 2]]);
        assert!(parse(&cloak()).unwrap().meshes[0].bones.is_empty());
    }

    #[test]
    fn truncations_and_an_extra_byte_are_errors() {
        for d in [skinned(), cloak()] {
            let mut longer = d.clone();
            longer.push(0);
            assert!(parse(&longer).is_err());
            for n in 0..d.len() {
                assert!(parse(&d[..n]).is_err(), "prefix of {n} of {} bytes", d.len());
            }
        }
        let mut longer = two();
        longer.push(0);
        assert!(parse(&longer).is_err());
    }

    #[test]
    fn the_first_mesh_of_a_two_mesh_file_is_a_whole_skinned_file() {
        // not a truncation error: the first mesh alone is itself a complete skinned file (so a cut at that point reads as one)
        let first = skinned().len();
        let d = two();
        for n in 0..d.len() {
            assert_eq!(parse(&d[..n]).is_ok(), n == first, "prefix of {n} bytes");
        }
        assert_eq!(parse(&d[..first]).unwrap().layout, C3dcLayout::Skinned);
    }

    #[test]
    fn a_file_two_layouts_read_is_refused_as_ambiguous() {
        // the decision function: exactly one layout may read a file
        assert_eq!(pick_layout(&[true, false, false]), Pick::One(0));
        assert_eq!(pick_layout(&[false, true, false]), Pick::One(1));
        assert_eq!(pick_layout(&[false, false, true]), Pick::One(2));
        assert_eq!(pick_layout(&[false, false, false]), Pick::None);
        assert_eq!(pick_layout(&[true, false, true]), Pick::Ambiguous);
        assert_eq!(pick_layout(&[true, true, true]), Pick::Ambiguous);
        // a mesh with no bones, no vertices and no faces: skinned reads "0 bones, 0 vertices, 0 faces"; read as a cloak the same bytes are
        // "0 vertices, 0 faces" followed by 4 bytes left over, so only one layout reads them and the file is not ambiguous
        let d = c3dc_bytes(b"", &[], &[], &[]);
        assert_eq!(parse(&d).unwrap().layout, C3dcLayout::Skinned);
    }

    #[test]
    fn a_none_error_is_the_skinned_one() {
        // a cloak with one byte too many: the skinned and the cloak attempts fail differently
        let mut d = cloak();
        d.push(0);
        let skinned_err = whole(&d, "t.3dc", 1, true).unwrap_err().to_string();
        let cloak_err = whole(&d, "t.3dc", 1, false).unwrap_err().to_string();
        assert_ne!(skinned_err, cloak_err);
        assert_eq!(parse(&d).unwrap_err().to_string(), skinned_err);
    }

    #[test]
    fn try_to_bytes_refuses_a_mesh_another_layout_also_reads() {
        // skinned, empty texture, one bone whose element 10 holds bits 18, two vertices, no faces: also reads as a cloak
        let mut bone = [0f32; 16];
        bone[10] = f32::from_bits(18);
        let v = parse(&skinned()).unwrap().meshes[0].vertices[0].clone();
        let m = C3dcMesh { texture: Vec::new(), bones: vec![bone], vertices: vec![v.clone(), v], faces: Vec::new() };
        let c = C3dc { layout: C3dcLayout::Skinned, meshes: vec![m] };
        assert!(c.try_to_bytes().is_err());
        // an ordinary mesh still writes
        assert_eq!(parse(&skinned()).unwrap().try_to_bytes().unwrap(), skinned());
    }

    #[test]
    fn a_non_zero_pad_survives() {
        let d = c3dc_bytes(b"t\0", &[identity_bits()], &[vert(0, 0, 0xBEEF), vert(0, 0, 7)], &[]);
        let m = parse(&d).unwrap();
        assert_eq!((m.meshes[0].vertices[0].pad, m.meshes[0].vertices[1].pad), (0xBEEF, 7));
        assert_eq!(m.to_bytes(), d);
    }

    #[test]
    fn texture_bytes_after_the_nul_survive() {
        let t = b"skin.dds\0junk\xB0\xA1\0";
        let d = c3dc_bytes(t, &[identity_bits()], &[vert(0, 0, 0)], &[]);
        let m = parse(&d).unwrap();
        assert_eq!(m.meshes[0].texture, t.to_vec());
        assert_eq!(m.texture_name(), "skin.dds");
        assert_eq!(m.to_bytes(), d);
    }

    #[test]
    fn nan_and_odd_float_bits_survive() {
        let mut m = parse(&skinned()).unwrap();
        m.meshes[0].bones[0][5] = f32::from_bits(0xFFC0_0000);
        m.meshes[0].vertices[0].normal[1] = f32::from_bits(0x7FA0_0001);
        let d = m.to_bytes();
        let back = parse(&d).unwrap();
        assert_eq!(back.meshes[0].bones[0][5].to_bits(), 0xFFC0_0000);
        assert_eq!(back.meshes[0].vertices[0].normal[1].to_bits(), 0x7FA0_0001);
        assert_eq!(back.to_bytes(), d);
    }

    #[test]
    fn a_count_that_cannot_fit_is_refused_without_allocating() {
        let mut d = c3dc_cloak_bytes(b"", &[], &[]);
        // texture length 0, then the vertex count
        d[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(parse(&d).is_err());
        d[4..8].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(parse(&d).is_err());
        let mut s = c3dc_bytes(b"", &[], &[], &[]);
        s[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(parse(&s).is_err());
        // a texture length that runs past the end
        assert!(parse(&[0xFF, 0xFF, 0xFF, 0x7F]).is_err());
    }

    #[test]
    fn a_mesh_that_cannot_be_written_is_refused() {
        let mut m = parse(&skinned()).unwrap();
        m.layout = C3dcLayout::Cloak;
        assert!(m.try_to_bytes().is_err(), "a cloak with a bone block");
        m.layout = C3dcLayout::SkinnedWithSecondMesh;
        assert!(m.try_to_bytes().is_err(), "second-mesh layout with one mesh");
        m.layout = C3dcLayout::Skinned;
        m.meshes[0].bones = vec![[0.0; 16]; BONE_LIMIT + 1];
        assert!(m.try_to_bytes().is_err());
    }
}
