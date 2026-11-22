//! Byte builders for the animation formats, written from the documented layouts without using `shaiya-clientdata`: an independent writer,
//! so a reader/writer round trip is tested against bytes it did not make. Synthetic data only.

/// One bone of a `.ANI`: parent, the 16 matrix elements as raw bits, rotation keys `(key, x y z w)`, translation keys `(key, x y z)`.
#[derive(Clone, Debug)]
pub struct BoneSpec {
    pub parent: i32,
    pub matrix: [u32; 16],
    pub rot: Vec<(i32, [f32; 4])>,
    pub trans: Vec<(i32, [f32; 3])>,
}

/// The bytes of a `.ANI`: `i32 first field, i32 end key, u16 bone count`, then per bone `i32 parent, f32[16], i32 nRot, keys, i32 nTrans, keys`.
pub fn ani_bytes(first_field: i32, end_key: i32, bones: &[BoneSpec]) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&first_field.to_le_bytes());
    d.extend_from_slice(&end_key.to_le_bytes());
    d.extend_from_slice(&(bones.len() as u16).to_le_bytes());
    for b in bones {
        d.extend_from_slice(&b.parent.to_le_bytes());
        for m in b.matrix {
            d.extend_from_slice(&m.to_le_bytes());
        }
        d.extend_from_slice(&(b.rot.len() as i32).to_le_bytes());
        for (key, q) in &b.rot {
            d.extend_from_slice(&key.to_le_bytes());
            for c in q {
                d.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
        d.extend_from_slice(&(b.trans.len() as i32).to_le_bytes());
        for (key, v) in &b.trans {
            d.extend_from_slice(&key.to_le_bytes());
            for c in v {
                d.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
    }
    d
}

/// The identity matrix as raw bits (row-major, translation in elements 12..15).
pub fn identity_bits() -> [u32; 16] {
    translation_bits(0.0, 0.0, 0.0)
}

/// The identity matrix with the translation `(x, y, z)`, as raw bits.
pub fn translation_bits(x: f32, y: f32, z: f32) -> [u32; 16] {
    let mut m = [0f32; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m.map(f32::to_bits)
}

/// A three-bone chain: the root at the origin, bone 1 at y = 1 and bone 2 at y = 2 (each the child of the one before). Bone 1 turns 90
/// degrees about z from key 0 to `end_key`; the other bones have no keys.
pub fn arm_ani(end_key: i32) -> Vec<u8> {
    let (s, c) = (std::f32::consts::FRAC_PI_4.sin(), std::f32::consts::FRAC_PI_4.cos());
    let bone = |parent: i32, y: f32, rot: Vec<(i32, [f32; 4])>| BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot, trans: Vec::new() };
    ani_bytes(
        0,
        end_key,
        &[bone(-1, 0.0, Vec::new()), bone(0, 1.0, vec![(0, [0.0, 0.0, 0.0, 1.0]), (end_key, [0.0, 0.0, s, c])]), bone(1, 2.0, Vec::new())],
    )
}

/// One vertex of a `.3DC` (40 bytes in the file): position, weight, the two bone indices, the pad field, normal, texture coordinates.
#[derive(Clone, Copy, Debug)]
pub struct VertSpec {
    pub pos: [f32; 3],
    pub weight: f32,
    pub bone1: u8,
    pub bone2: u8,
    pub pad: u16,
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

fn c3dc_part(d: &mut Vec<u8>, texture: &[u8], bones: Option<&[[u32; 16]]>, vertices: &[VertSpec], faces: &[[u16; 3]]) {
    d.extend_from_slice(&(texture.len() as u32).to_le_bytes());
    d.extend_from_slice(texture);
    if let Some(bones) = bones {
        d.extend_from_slice(&(bones.len() as i32).to_le_bytes());
        for b in bones {
            for m in b {
                d.extend_from_slice(&m.to_le_bytes());
            }
        }
    }
    d.extend_from_slice(&(vertices.len() as i32).to_le_bytes());
    for v in vertices {
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
    d.extend_from_slice(&(faces.len() as i32).to_le_bytes());
    for f in faces {
        for i in f {
            d.extend_from_slice(&i.to_le_bytes());
        }
    }
}

/// The bytes of a skinned `.3DC`: `i32 length + texture bytes, i32 bone count, matrices, i32 vertex count, vertices, i32 face count, faces`.
/// Two parts back to back (concatenate two results) are the second-mesh layout.
pub fn c3dc_bytes(texture: &[u8], bones: &[[u32; 16]], vertices: &[VertSpec], faces: &[[u16; 3]]) -> Vec<u8> {
    let mut d = Vec::new();
    c3dc_part(&mut d, texture, Some(bones), vertices, faces);
    d
}

/// The bytes of a cloak `.3DC`: the skinned layout without the bone block.
pub fn c3dc_cloak_bytes(texture: &[u8], vertices: &[VertSpec], faces: &[[u16; 3]]) -> Vec<u8> {
    let mut d = Vec::new();
    c3dc_part(&mut d, texture, None, vertices, faces);
    d
}

/// A skinned mesh for the three-bone chain of [`arm_ani`]: two boxes along y (y 0 to 1 bound to bones 0 and 1, y 1 to 2 bound to bones 1
/// and 2); the lower corners of a box take weight 1.0 of the first bone, the upper corners 0.5. The three matrices are translations by -y
/// (the inverse of the bones' rest positions). 16 vertices, 24 faces.
pub fn arm_mesh_3dc() -> Vec<u8> {
    let bones = [translation_bits(0.0, 0.0, 0.0), translation_bits(0.0, -1.0, 0.0), translation_bits(0.0, -2.0, 0.0)];
    let mut vertices = Vec::new();
    let mut faces: Vec<[u16; 3]> = Vec::new();
    for (b, y0) in [(0u8, 0.0f32), (1, 1.0)] {
        let base = vertices.len() as u16;
        for i in 0..8u16 {
            let (x, y, z) = (if i & 1 == 0 { -0.1 } else { 0.1 }, if i & 2 == 0 { 0.0 } else { 1.0 }, if i & 4 == 0 { -0.1 } else { 0.1 });
            vertices.push(VertSpec { pos: [x, y0 + y, z], weight: if y == 0.0 { 1.0 } else { 0.5 }, bone1: b, bone2: b + 1, pad: 0, normal: [0.0, 1.0, 0.0], uv: [x + 0.5, z + 0.5] });
        }
        // 12 triangles: two per side of the box (corner index bits: x = 1, y = 2, z = 4)
        for q in [[0u16, 1, 3, 2], [4, 6, 7, 5], [0, 2, 6, 4], [1, 5, 7, 3], [0, 4, 5, 1], [2, 3, 7, 6]] {
            faces.push([base + q[0], base + q[1], base + q[2]]);
            faces.push([base + q[0], base + q[2], base + q[3]]);
        }
    }
    c3dc_bytes(b"arm.tga\0", &bones, &vertices, &faces)
}
