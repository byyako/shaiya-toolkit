//! `.smod` static meshes (`Entity/Building`, `Entity/Shape`, `Entity/Tree`): the buildings, shapes and trees a `.wld` places.
//! Layout as observed in client files.
//!
//! ```text
//! f32 centre[3]; i32 unk; f32 min[3], max[3]
//! i32 nModules; module: i32-length string texture, i32 nVert, vertex[nVert] (36 bytes), i32 nFace, face[nFace] (3 x u16)
//! f32 box2 min[3], max[3]; i32 nExtra (a flag; one collision mesh follows when set: i32 nPoint, f32[3] x nPoint, i32 nFace, face[])
//! ```

use crate::error::{Cur, Result};

/// 36 bytes: position, normal, colour (0xFFFFFFFF in every file), uv.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmodVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub colour: u32,
    pub uv: [f32; 2],
}

/// One textured part of a model.
#[derive(Clone, Debug, PartialEq)]
pub struct SmodModule {
    /// Texture name in `Entity/Texture` (the file says `.tga`, the cleaned archive stores `.dds`).
    pub texture: String,
    pub vertices: Vec<SmodVertex>,
    pub faces: Vec<[u16; 3]>,
}

/// An untextured collision mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct SmodCollision {
    pub points: Vec<[f32; 3]>,
    pub faces: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Smod {
    pub centre: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub modules: Vec<SmodModule>,
    pub box2: ([f32; 3], [f32; 3]),
    pub collision: Vec<SmodCollision>,
}

impl Smod {
    pub fn parse(data: &[u8], file: &str) -> Result<Smod> {
        let mut r = Cur::new(data, file);
        let centre = r.f3("centre")?;
        r.i32("unknown")?;
        let (min, max) = (r.f3("bounding box")?, r.f3("bounding box")?);
        let nm = r.count(12, 10_000, "module count")?;
        let mut modules = Vec::with_capacity(nm);
        for _ in 0..nm {
            let texture = r.str_i32("texture name")?;
            let nv = r.count(36, 10_000_000, "vertex count")?;
            let raw = r.bytes(nv * 36, "vertices")?;
            let f = |o: usize, c: &[u8]| f32::from_le_bytes(c[o..o + 4].try_into().unwrap());
            let vertices = raw
                .chunks_exact(36)
                .map(|c| SmodVertex { pos: [f(0, c), f(4, c), f(8, c)], normal: [f(12, c), f(16, c), f(20, c)], colour: u32::from_le_bytes(c[24..28].try_into().unwrap()), uv: [f(28, c), f(32, c)] })
                .collect();
            let faces = faces(&mut r, "faces")?;
            modules.push(SmodModule { texture, vertices, faces });
        }
        let box2 = (r.f3("second box")?, r.f3("second box")?);
        let extra = r.count(0, 64, "collision flag")?;
        let mut collision = Vec::new();
        for _ in 0..extra {
            let np = r.count(12, 10_000_000, "collision point count")?;
            let raw = r.bytes(np * 12, "collision points")?;
            let points = raw
                .chunks_exact(12)
                .map(|c| {
                    let f = |o: usize| f32::from_le_bytes(c[o..o + 4].try_into().unwrap());
                    [f(0), f(4), f(8)]
                })
                .collect();
            collision.push(SmodCollision { points, faces: faces(&mut r, "collision faces")? });
        }
        r.end("model")?;
        Ok(Smod { centre, min, max, modules, box2, collision })
    }

    /// Vertices and faces over all modules.
    pub fn counts(&self) -> (usize, usize) {
        (self.modules.iter().map(|m| m.vertices.len()).sum(), self.modules.iter().map(|m| m.faces.len()).sum())
    }
}

fn faces(r: &mut Cur, what: &str) -> Result<Vec<[u16; 3]>> {
    let n = r.count(6, 10_000_000, what)?;
    let raw = r.bytes(n * 6, what)?;
    Ok(raw.chunks_exact(6).map(|c| [u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]), u16::from_le_bytes([c[4], c[5]])]).collect())
}

/// Orientation of a placed model: the columns of the rotation that takes model space to world space. `dir` is the model's +z
/// (forward), `up` its +y; +x is `up x dir`, which makes a left-handed frame like the client's (a map yaw of `a` gives
/// `dir = (sin a, 0, cos a)`, `x = (cos a, 0, -sin a)`). Both vectors are unit length and perpendicular in every EP4 map (no
/// scale is stored), but they are re-orthogonalised here so a hand-edited file cannot shear a model.
pub fn placement_basis(dir: [f32; 3], up: [f32; 3]) -> [[f32; 3]; 3] {
    fn norm(v: [f32; 3]) -> [f32; 3] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l > 1e-6 && l.is_finite() {
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            [0.0; 3]
        }
    }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    }
    let z = norm(dir);
    let y = norm(up);
    if z == [0.0; 3] || y == [0.0; 3] {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let x = norm(cross(y, z));
    if x == [0.0; 3] {
        // dir and up are parallel: keep the up direction and pick any forward
        let alt = if y[1].abs() > 0.9 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };
        let x = norm(cross(y, alt));
        let z = cross(x, y);
        return [x, y, z];
    }
    let y = cross(z, x);
    [x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut d = Vec::new();
        let f = |d: &mut Vec<u8>, v: f32| d.extend_from_slice(&v.to_le_bytes());
        for v in [0.0f32, 1.0, 0.0] {
            f(&mut d, v);
        }
        d.extend_from_slice(&0i32.to_le_bytes());
        for v in [-1.0f32, 0.0, -1.0, 1.0, 2.0, 1.0] {
            f(&mut d, v);
        }
        d.extend_from_slice(&1i32.to_le_bytes()); // one module
        let name = b"wall.tga\0";
        d.extend_from_slice(&(name.len() as i32).to_le_bytes());
        d.extend_from_slice(name);
        d.extend_from_slice(&3i32.to_le_bytes());
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for v in p {
                f(&mut d, v);
            }
            for v in [0.0f32, 0.0, 1.0] {
                f(&mut d, v);
            }
            d.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            f(&mut d, p[0]);
            f(&mut d, p[1]);
        }
        d.extend_from_slice(&1i32.to_le_bytes());
        for i in [0u16, 1, 2] {
            d.extend_from_slice(&i.to_le_bytes());
        }
        for v in [-1.0f32, 0.0, -1.0, 1.0, 2.0, 1.0] {
            f(&mut d, v);
        }
        d.extend_from_slice(&0i32.to_le_bytes()); // no collision
        d
    }

    #[test]
    fn parses_a_small_model_and_rejects_damage() {
        let d = sample();
        let m = Smod::parse(&d, "t.smod").unwrap();
        assert_eq!(m.modules.len(), 1);
        assert_eq!(m.modules[0].texture, "wall.tga");
        assert_eq!(m.counts(), (3, 1));
        assert_eq!(m.max, [1.0, 2.0, 1.0]);
        assert!(m.collision.is_empty());
        // trailing bytes and every truncation are errors, never panics
        let mut longer = d.clone();
        longer.push(0);
        assert!(Smod::parse(&longer, "t.smod").is_err());
        for n in 0..d.len() {
            assert!(Smod::parse(&d[..n], "t.smod").is_err(), "prefix of {n} bytes");
        }
    }

    #[test]
    fn basis_is_a_left_handed_yaw_for_flat_models() {
        let a = 0.7f32;
        let b = placement_basis([a.sin(), 0.0, a.cos()], [0.0, 1.0, 0.0]);
        assert!((b[0][0] - a.cos()).abs() < 1e-6 && (b[0][2] + a.sin()).abs() < 1e-6, "x = (cos, 0, -sin): {:?}", b[0]);
        assert_eq!(b[1], [0.0, 1.0, 0.0]);
        assert!((b[2][0] - a.sin()).abs() < 1e-6);
        // identity for the usual (0,0,1) / (0,1,0)
        assert_eq!(placement_basis([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        // degenerate input still gives a unit frame
        for (d, u) in [([0.0; 3], [0.0; 3]), ([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]), ([f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0])] {
            let b = placement_basis(d, u);
            for c in b {
                let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                assert!((l - 1.0).abs() < 1e-5, "{d:?} {u:?} {b:?}");
            }
        }
    }
}
