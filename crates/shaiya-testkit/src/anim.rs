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

/// One vertex of a `.VANI` (36 bytes in the file): position, normal, diffuse colour, texture coordinates.
#[derive(Clone, Copy, Debug)]
pub struct VaniVertSpec {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub diffuse: u32,
    pub uv: [f32; 2],
}

/// One item of a `.VANI`: texture bytes, the faces shared by every step and the vertices of all steps, step by step (the number per step is
/// `vertices.len() / step_count`).
#[derive(Clone, Debug)]
pub struct VaniItemSpec {
    pub texture: Vec<u8>,
    pub faces: Vec<[u16; 3]>,
    pub vertices: Vec<VaniVertSpec>,
}

/// A collision mesh: points and faces.
#[derive(Clone, Debug)]
pub struct CollisionSpec {
    pub points: Vec<[f32; 3]>,
    pub faces: Vec<[u16; 3]>,
}

/// A `.VANI` to build: sphere, box, steps, `frame_ms`, items, the second box and an optional collision mesh (points, faces).
#[derive(Clone, Debug)]
pub struct VaniSpec {
    pub centre: [f32; 3],
    pub radius: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub step_count: i32,
    pub frame_ms: i32,
    pub items: Vec<VaniItemSpec>,
    pub box2: ([f32; 3], [f32; 3]),
    pub collision: Option<CollisionSpec>,
}

fn put_f32s(d: &mut Vec<u8>, v: &[f32]) {
    for c in v {
        d.extend_from_slice(&c.to_bits().to_le_bytes());
    }
}

fn put_faces(d: &mut Vec<u8>, faces: &[[u16; 3]]) {
    d.extend_from_slice(&(faces.len() as i32).to_le_bytes());
    for f in faces {
        for i in f {
            d.extend_from_slice(&i.to_le_bytes());
        }
    }
}

/// The bytes of a `.VANI`: `f32 sphere centre[3], radius, box min[3], max[3]; i32 item count, step count, frame_ms`; per item `i32 length +
/// texture, faces, i32 vertices per step, vertices`; `f32 second box`; `i32 collision flag` (0, or 1 and a collision mesh). The vertex
/// count of an item must be a multiple of the step count.
pub fn vani_bytes(s: &VaniSpec) -> Vec<u8> {
    let mut d = Vec::new();
    put_f32s(&mut d, &s.centre);
    put_f32s(&mut d, &[s.radius]);
    put_f32s(&mut d, &s.min);
    put_f32s(&mut d, &s.max);
    for n in [s.items.len() as i32, s.step_count, s.frame_ms] {
        d.extend_from_slice(&n.to_le_bytes());
    }
    for it in &s.items {
        d.extend_from_slice(&(it.texture.len() as u32).to_le_bytes());
        d.extend_from_slice(&it.texture);
        put_faces(&mut d, &it.faces);
        let per_step = if s.step_count > 0 { it.vertices.len() / s.step_count as usize } else { 0 };
        assert_eq!(per_step * s.step_count.max(0) as usize, it.vertices.len(), "vertices must be a whole number of steps");
        d.extend_from_slice(&(per_step as i32).to_le_bytes());
        for v in &it.vertices {
            put_f32s(&mut d, &v.pos);
            put_f32s(&mut d, &v.normal);
            d.extend_from_slice(&v.diffuse.to_le_bytes());
            put_f32s(&mut d, &v.uv);
        }
    }
    put_f32s(&mut d, &s.box2.0);
    put_f32s(&mut d, &s.box2.1);
    match &s.collision {
        None => d.extend_from_slice(&0i32.to_le_bytes()),
        Some(c) => {
            d.extend_from_slice(&1i32.to_le_bytes());
            d.extend_from_slice(&(c.points.len() as i32).to_le_bytes());
            for p in &c.points {
                put_f32s(&mut d, p);
            }
            put_faces(&mut d, &c.faces);
        }
    }
    d
}

/// A `.VANI` with one item: a unit box (8 vertices, 12 triangles) whose top four vertices rise by `step / 10` at each of `steps` steps.
pub fn box_vani(steps: i32, frame_ms: i32) -> Vec<u8> {
    let mut faces: Vec<[u16; 3]> = Vec::new();
    for q in [[0u16, 1, 3, 2], [4, 6, 7, 5], [0, 2, 6, 4], [1, 5, 7, 3], [0, 4, 5, 1], [2, 3, 7, 6]] {
        faces.push([q[0], q[1], q[2]]);
        faces.push([q[0], q[2], q[3]]);
    }
    let mut vertices = Vec::new();
    for step in 0..steps {
        for i in 0..8u16 {
            let (x, y, z) = (if i & 1 == 0 { 0.0 } else { 1.0 }, if i & 2 == 0 { 0.0 } else { 1.0 }, if i & 4 == 0 { 0.0 } else { 1.0 });
            let lift = if y > 0.5 { step as f32 / 10.0 } else { 0.0 };
            vertices.push(VaniVertSpec { pos: [x, y + lift, z], normal: [0.0, 1.0, 0.0], diffuse: 0xFFFF_FFFF, uv: [x, z] });
        }
    }
    vani_bytes(&VaniSpec {
        centre: [0.5, 0.5, 0.5],
        radius: 0.866,
        min: [0.0, 0.0, 0.0],
        max: [1.0, 1.0, 1.0],
        step_count: steps,
        frame_ms,
        items: vec![VaniItemSpec { texture: b"box.tga\0".to_vec(), faces, vertices }],
        box2: ([0.0; 3], [1.0; 3]),
        collision: None,
    })
}

/// The 108 bytes of a `.MANI` with typical values: `tick_ms`, `move_on` and `move_pause_ms` (two `u16` at offset 80), and the last field as raw
/// bits (`500` gives the integer form, `10.0f32.to_bits()` the float form). Every other field takes a plain value.
pub fn mani_bytes(tick_ms: i32, move_on: u16, move_pause_ms: u16, distance_bits: u32) -> Vec<u8> {
    let mut d = Vec::with_capacity(108);
    let f = |d: &mut Vec<u8>, v: &[f32]| put_f32s(d, v);
    d.extend_from_slice(&tick_ms.to_le_bytes());
    d.extend_from_slice(&0i32.to_le_bytes()); // swing_on
    f(&mut d, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0]); // swing_axis, swing_limit, swing_step, swing_unused
    d.extend_from_slice(&80i32.to_le_bytes()); // swing_pause_ms
    d.extend_from_slice(&0i32.to_le_bytes()); // orbit_on
    f(&mut d, &[1.0, 0.0, 0.0, 0.0, 0.0]); // orbit_axis, orbit_step, orbit_radius
    d.extend_from_slice(&0i32.to_le_bytes()); // spin_on
    f(&mut d, &[0.0, 0.0, 1.0, -0.01745]); // spin_axis, spin_step
    d.extend_from_slice(&move_on.to_le_bytes());
    d.extend_from_slice(&move_pause_ms.to_le_bytes());
    f(&mut d, &[1.0, 0.0, 0.0, 0.0, 1.01]); // move_dir, move_speed, move_accel
    d.extend_from_slice(&distance_bits.to_le_bytes());
    assert_eq!(d.len(), 108);
    d
}
