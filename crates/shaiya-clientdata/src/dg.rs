//! `.dg` dungeon geometry (`world/dungeon/<name>.dg`): an octree of textured, lightmapped meshes plus a collision mesh per
//! node. Layout: `docs/formats.md` ".DG" (`SDungeonScene::Load`, `FUN_0058fba0`), ported from `tools/formats.py parse_dg`.

use crate::error::{Cur, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// FVF 0x252 vertex, 44 bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DgVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub colour: u32,
    pub uv: [f32; 2],
    pub lightmap_uv: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct DgMesh {
    /// Lightmap number: `<dg name>_L<n>.dds` next to the `.dg`.
    pub lightmap: i32,
    pub vertices: Vec<DgVertex>,
    pub faces: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DgGroup {
    /// Index into [`Dg::textures`].
    pub texture: i32,
    pub meshes: Vec<DgMesh>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DgCollision {
    pub points: Vec<[f32; 3]>,
    pub faces: Vec<[u16; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DgNode {
    pub centre: [f32; 3],
    pub box_a: Aabb3,
    pub box_b: Aabb3,
    pub groups: Vec<DgGroup>,
    pub collision: Option<DgCollision>,
    pub children: Vec<DgNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dg {
    pub bounds: Aabb3,
    /// Texture names in `Entity/Texture`.
    pub textures: Vec<String>,
    pub lightmaps: i32,
    pub root: Option<DgNode>,
}

/// Counts of what a `.dg` holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DgStats {
    pub nodes: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub faces: usize,
    pub collision_points: usize,
    pub collision_faces: usize,
    pub depth: usize,
}

/// One draw batch: a texture and a lightmap, with merged buffers (indices are `u32` into `vertices`).
#[derive(Clone, Debug, PartialEq)]
pub struct DgBatch {
    pub texture: i32,
    pub lightmap: i32,
    pub vertices: Vec<DgVertex>,
    pub indices: Vec<u32>,
}

/// Everything flattened for drawing and ground queries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DgFlat {
    pub batches: Vec<DgBatch>,
    pub collision_points: Vec<[f32; 3]>,
    pub collision_indices: Vec<u32>,
}

const MAX_DEPTH: usize = 40;

impl Dg {
    pub fn parse(data: &[u8], file: &str) -> Result<Dg> {
        let mut r = Cur::new(data, file);
        let bounds = Aabb3 { min: r.f3("bounds")?, max: r.f3("bounds")? };
        let nt = r.count(256, 100_000, "texture count")?;
        let mut textures = Vec::with_capacity(nt);
        for _ in 0..nt {
            textures.push(r.name256("texture name")?);
        }
        let lightmaps = r.i32("lightmap count")?;
        let root = if r.i32("root flag")? != 0 { Some(node(&mut r, 0)?) } else { None };
        r.end("dungeon")?;
        Ok(Dg { bounds, textures, lightmaps, root })
    }

    pub fn stats(&self) -> DgStats {
        fn walk(n: &DgNode, depth: usize, s: &mut DgStats) {
            s.nodes += 1;
            s.depth = s.depth.max(depth);
            for g in &n.groups {
                for m in &g.meshes {
                    s.meshes += 1;
                    s.vertices += m.vertices.len();
                    s.faces += m.faces.len();
                }
            }
            if let Some(c) = &n.collision {
                s.collision_points += c.points.len();
                s.collision_faces += c.faces.len();
            }
            for c in &n.children {
                walk(c, depth + 1, s);
            }
        }
        let mut s = DgStats::default();
        if let Some(r) = &self.root {
            walk(r, 0, &mut s);
        }
        s
    }

    /// Merge meshes by (texture, lightmap) and all collision meshes into single buffers.
    pub fn flatten(&self) -> DgFlat {
        use std::collections::BTreeMap;
        fn walk(n: &DgNode, batches: &mut BTreeMap<(i32, i32), DgBatch>, out: &mut DgFlat) {
            for g in &n.groups {
                for m in &g.meshes {
                    let b = batches.entry((g.texture, m.lightmap)).or_insert_with(|| DgBatch {
                        texture: g.texture,
                        lightmap: m.lightmap,
                        vertices: Vec::new(),
                        indices: Vec::new(),
                    });
                    let base = b.vertices.len() as u32;
                    b.vertices.extend_from_slice(&m.vertices);
                    for f in &m.faces {
                        b.indices.extend(f.iter().map(|&i| base + i as u32));
                    }
                }
            }
            if let Some(c) = &n.collision {
                let base = out.collision_points.len() as u32;
                out.collision_points.extend_from_slice(&c.points);
                for f in &c.faces {
                    out.collision_indices.extend(f.iter().map(|&i| base + i as u32));
                }
            }
            for c in &n.children {
                walk(c, batches, out);
            }
        }
        let mut out = DgFlat::default();
        let mut batches = BTreeMap::new();
        if let Some(r) = &self.root {
            walk(r, &mut batches, &mut out);
        }
        out.batches = batches.into_values().collect();
        out
    }
}

/// Map side the server uses for a dungeon: the larger of the `.dg` bound x and z (bounds start at 0), rounded up to a
/// multiple of 8. Holds for 6 of the 7 dungeons checked; map 3 uses 1024 for bounds 474 x 465, so treat it as a
/// suggestion (`docs/svmap-editor.md` section 3).
pub fn dungeon_map_size(bounds: &Aabb3) -> u32 {
    let m = bounds.max[0].max(bounds.max[2]).max(0.0).ceil() as u32;
    m.div_ceil(8) * 8
}

fn faces(r: &mut Cur, what: &str) -> Result<Vec<[u16; 3]>> {
    let n = r.count(6, 10_000_000, what)?;
    let raw = r.bytes(n * 6, what)?;
    Ok(raw
        .chunks_exact(6)
        .map(|c| [u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]), u16::from_le_bytes([c[4], c[5]])])
        .collect())
}

fn node(r: &mut Cur, depth: usize) -> Result<DgNode> {
    if depth > MAX_DEPTH {
        return Err(r.fail("octree deeper than 40 levels"));
    }
    let centre = r.f3("node centre")?;
    let box_a = Aabb3 { min: r.f3("node box")?, max: r.f3("node box")? };
    let box_b = Aabb3 { min: r.f3("node box")?, max: r.f3("node box")? };
    let ng = r.count(8, 10_000, "group count")?;
    let mut groups = Vec::with_capacity(ng);
    for _ in 0..ng {
        let texture = r.i32("group texture")?;
        let nm = r.count(8, 100_000, "mesh count")?;
        let mut meshes = Vec::with_capacity(nm);
        for _ in 0..nm {
            let lightmap = r.i32("mesh lightmap")?;
            let nv = r.count(44, 10_000_000, "vertex count")?;
            let raw = r.bytes(nv * 44, "vertices")?;
            let f = |o: usize, c: &[u8]| f32::from_le_bytes(c[o..o + 4].try_into().unwrap());
            let vertices = raw
                .chunks_exact(44)
                .map(|c| DgVertex {
                    pos: [f(0, c), f(4, c), f(8, c)],
                    normal: [f(12, c), f(16, c), f(20, c)],
                    colour: u32::from_le_bytes(c[24..28].try_into().unwrap()),
                    uv: [f(28, c), f(32, c)],
                    lightmap_uv: [f(36, c), f(40, c)],
                })
                .collect();
            let faces = faces(r, "faces")?;
            meshes.push(DgMesh { lightmap, vertices, faces });
        }
        groups.push(DgGroup { texture, meshes });
    }
    let collision = if r.i32("collision flag")? != 0 {
        let np = r.count(12, 10_000_000, "collision point count")?;
        let raw = r.bytes(np * 12, "collision points")?;
        let points = raw
            .chunks_exact(12)
            .map(|c| {
                let f = |o: usize| f32::from_le_bytes(c[o..o + 4].try_into().unwrap());
                [f(0), f(4), f(8)]
            })
            .collect();
        let faces = faces(r, "collision faces")?;
        Some(DgCollision { points, faces })
    } else {
        None
    };
    let mut children = Vec::new();
    for _ in 0..8 {
        if r.i32("child flag")? != 0 {
            children.push(node(r, depth + 1)?);
        }
    }
    Ok(DgNode { centre, box_a, box_b, groups, collision, children })
}
