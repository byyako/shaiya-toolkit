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
