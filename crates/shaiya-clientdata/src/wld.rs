//! `.wld` client maps (`world/<id>.wld`), parsed in full. Layout: `docs/formats.md` "World" (loader `FUN_004516f0`),
//! ported from `tools/formats.py parse_wld`. The reader must consume the file to its last byte.

use crate::error::{Cur, Error, Result};

/// Height of a terrain corner in world units = `raw * HEIGHT_SCALE + HEIGHT_OFFSET` (checked against NPC and portal
/// heights in `docs/svmap-editor.md` section 3).
pub const HEIGHT_SCALE: f32 = 0.02;
pub const HEIGHT_OFFSET: f32 = -200.0;
/// Distance between terrain corners in world units.
pub const CORNER_SPACING: f32 = 2.0;

/// The terrain grid of a field map: `n = size / 2 + 1` corners per side, index `cz * n + cx`.
#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    pub size: u32,
    pub n: usize,
    pub heights: Vec<i16>,
    /// Terrain texture per corner, index into [`Wld::textures`].
    pub tex_index: Vec<u8>,
}

impl Terrain {
    /// Height of corner (`cx`, `cz`), clamped into the grid.
    pub fn corner(&self, cx: usize, cz: usize) -> f32 {
        let cx = cx.min(self.n - 1);
        let cz = cz.min(self.n - 1);
        self.heights[cz * self.n + cx] as f32 * HEIGHT_SCALE + HEIGHT_OFFSET
    }

    /// Ground height at world (x, z): bilinear between the four corners of the 2-unit cell; positions outside the map
    /// are clamped to the edge.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let max = (self.n - 1) as f32;
        let fx = (x / CORNER_SPACING).clamp(0.0, max);
        let fz = (z / CORNER_SPACING).clamp(0.0, max);
        let (cx, cz) = (fx.floor() as usize, fz.floor() as usize);
        let (tx, tz) = (fx - cx as f32, fz - cz as f32);
        let h00 = self.corner(cx, cz);
        let h10 = self.corner(cx + 1, cz);
        let h01 = self.corner(cx, cz + 1);
        let h11 = self.corner(cx + 1, cz + 1);
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        a + (b - a) * tz
    }

    /// Lowest and highest corner height.
    pub fn height_range(&self) -> (f32, f32) {
        let lo = self.heights.iter().copied().min().unwrap_or(0);
        let hi = self.heights.iter().copied().max().unwrap_or(0);
        (lo as f32 * HEIGHT_SCALE + HEIGHT_OFFSET, hi as f32 * HEIGHT_SCALE + HEIGHT_OFFSET)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainTexture {
    /// File name in `Terrain/Detail` (e.g. `r2_snowbase.dds`).
    pub name: String,
    pub tiling: f32,
    /// Footstep sound name; picks the walk sound set.
    pub footstep: String,
}

/// `i32 nameIndex, f32 pos[3], f32 dir[3], f32 up[3]` (a building, shape, tree, ... placement; index -1 = skip).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub index: i32,
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    pub up: [f32; 3],
}

/// A list of names (fixed 256-byte buffers) followed by placements that refer to them by index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlacedList {
    pub names: Vec<String>,
    pub items: Vec<Placement>,
}

/// `i32 index1, i32 index2, f32 pos[3], dir[3], up[3]` (44 bytes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mani {
    pub index1: i32,
    pub index2: i32,
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    pub up: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ManiList {
    pub names: Vec<String>,
    pub items: Vec<Mani>,
}

/// A placed effect: `f32 pos[3], dir[3], up[3], i32 sequence`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectPlacement {
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    pub up: [f32; 3],
    pub sequence: i32,
}

/// An animated object (76 bytes): index, nine floats (pos, dir, up), nine 4-byte parameters kept raw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obj {
    pub index: i32,
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    pub up: [f32; 3],
    pub params: [u32; 9],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectList {
    pub names: Vec<String>,
    pub items: Vec<Obj>,
    /// Linked pairs (every EP4 map has none).
    pub pairs: Vec<[Obj; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BgmZone {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
    pub bgm_index: i32,
    pub flag: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WavZone {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub wavs: Vec<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundPoint {
    pub wav: i32,
    pub pos: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sound {
    pub bgm_names: Vec<String>,
    pub bgm_zones: Vec<BgmZone>,
    pub wav_names: Vec<String>,
    pub wav_zones: Vec<WavZone>,
    pub points: Vec<SoundPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrientedBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
}

/// A client portal (556 bytes). The svmap portal at the same index stands at the centre (x, z) of the box and its min y.
#[derive(Clone, Debug, PartialEq)]
pub struct WldPortal {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
    pub name: String,
    /// Destination name shown in the UI.
    pub dest_name: String,
    pub country: i32,
    pub level_min: i16,
    pub level_max: i16,
    pub dest_map: i32,
    pub dest_pos: [f32; 3],
}

impl WldPortal {
    /// Where the matching svmap portal stands: box centre (x, z), box min y.
    pub fn svmap_pos(&self) -> [f32; 3] {
        [(self.min[0] + self.max[0]) * 0.5, self.min[1], (self.min[2] + self.max[2]) * 0.5]
    }
}

/// One box of a zone group (36 bytes): box, rotY, `i32 country`, `i32 attr`. A zone group is a bind area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
    pub country: i32,
    pub attr: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneGroup {
    pub boxes: Vec<ZoneBox>,
}

/// A named area (548 bytes); its order is the line index in `world/<id>.txt`. `mode` 2 = picture caption `tga`.
#[derive(Clone, Debug, PartialEq)]
pub struct WldArea {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub rot_y: f32,
    pub name: String,
    pub tga: String,
    pub mode: i32,
    pub extra: i32,
}

/// An NPC record: type, type id, first point and angle, then more path points.
#[derive(Clone, Debug, PartialEq)]
pub struct WldNpc {
    pub kind: i32,
    pub type_id: i32,
    pub pos: [f32; 3],
    pub angle: f32,
    pub extra: Vec<[f32; 3]>,
}

impl WldNpc {
    /// Number of points of the matching svmap NPC (the first one plus the extra ones).
    pub fn point_count(&self) -> usize {
        1 + self.extra.len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WldKind {
    Field,
    Dungeon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wld {
    pub kind: WldKind,
    /// Terrain (fields only).
    pub terrain: Option<Terrain>,
    pub textures: Vec<TerrainTexture>,
    /// `.wtr` water name in `Entity/Water` (fields only; may be empty).
    pub water: String,
    /// Dungeon geometry name (dungeons only), a `.dg` in `world/dungeon`.
    pub dg: String,
    pub building: PlacedList,
    pub shape: PlacedList,
    pub tree: PlacedList,
    pub grass: PlacedList,
    pub vani: PlacedList,
    pub vani2: PlacedList,
    pub dungeon: PlacedList,
    pub mani: ManiList,
    pub effect_file: String,
    pub effects: Vec<EffectPlacement>,
    pub object: ObjectList,
    pub object2: PlacedList,
    pub sound: Sound,
    pub boxes: Vec<OrientedBox>,
    pub portals: Vec<WldPortal>,
    /// Zone groups = the server's bind areas, in the same order.
    pub zone_groups: Vec<ZoneGroup>,
    pub areas: Vec<WldArea>,
    /// The record count the file declares for the NPC list (records plus their extra points).
    pub npc_declared: u32,
    pub npcs: Vec<WldNpc>,
    /// Sky dome bitmap and two cloud textures (fields only).
    pub sky: Vec<String>,
    /// Three colours, each f32 r, g, b in 0..255.
    pub colours: [[f32; 3]; 3],
    pub fog_near: f32,
    pub fog_far: f32,
}

impl Wld {
    /// Map side for fields (`None` for dungeons: their size comes from the `.dg` bounds, see `dg::dungeon_map_size`).
    pub fn field_size(&self) -> Option<u32> {
        self.terrain.as_ref().map(|t| t.size)
    }

    pub fn parse(data: &[u8], file: &str) -> Result<Wld> {
        let mut r = Cur::new(data, file);
        let kind_raw = r.bytes(4, "map type")?;
        let kind = match kind_raw {
            b"FLD\0" => WldKind::Field,
            b"DUN\0" => WldKind::Dungeon,
            other => return Err(Error::Format { file: file.into(), what: format!("map type {other:?} is neither FLD nor DUN"), offset: 0 }),
        };
        let mut terrain = None;
        let mut textures = Vec::new();
        let mut water = String::new();
        let mut dg = String::new();
        if kind == WldKind::Field {
            let at = r.p;
            let size = r.i32("map size")?;
            if size <= 0 || size % 2 != 0 || size > 8192 {
                r.p = at;
                return Err(r.fail(format!("map size {size} is not an even number up to 8192")));
            }
            let n = size as usize / 2 + 1;
            let cells = n * n;
            if cells * 3 > r.left() {
                return Err(r.fail(format!("terrain grid {n}x{n} does not fit in the file")));
            }
            let raw = r.bytes(cells * 2, "terrain heights")?;
            let heights = raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            let tex_index = r.bytes(cells, "terrain texture indices")?.to_vec();
            terrain = Some(Terrain { size: size as u32, n, heights, tex_index });
            let nt = r.count(516, 256, "terrain texture count")?;
            for _ in 0..nt {
                let name = r.name256("terrain texture name")?;
                let tiling = r.f32("terrain texture tiling")?;
                let footstep = r.name256("terrain footstep name")?;
                textures.push(TerrainTexture { name, tiling, footstep });
            }
            water = r.name256("water name")?;
        } else {
            dg = r.name256("dungeon name")?;
        }
        let building = placed(&mut r, "building")?;
        let shape = placed(&mut r, "shape")?;
        let tree = placed(&mut r, "tree")?;
        let grass = placed(&mut r, "grass")?;
        let vani = placed(&mut r, "vani")?;
        let vani2 = placed(&mut r, "vani2")?;
        let dungeon = placed(&mut r, "dungeon")?;

        let mut mani = ManiList { names: names(&mut r, "mani names")?, items: Vec::new() };
        let n = r.count(44, 10_000_000, "mani count")?;
        for _ in 0..n {
            mani.items.push(Mani {
                index1: r.i32("mani index")?,
                index2: r.i32("mani index")?,
                pos: r.f3("mani")?,
                dir: r.f3("mani")?,
                up: r.f3("mani")?,
            });
        }

        let effect_file = r.name256("effect file")?;
        let n = r.count(40, 10_000_000, "effect count")?;
        let mut effects = Vec::with_capacity(n);
        for _ in 0..n {
            effects.push(EffectPlacement { pos: r.f3("effect")?, dir: r.f3("effect")?, up: r.f3("effect")?, sequence: r.i32("effect sequence")? });
        }

        let mut object = ObjectList { names: names(&mut r, "object names")?, ..Default::default() };
        let n = r.count(76, 10_000_000, "object count")?;
        for _ in 0..n {
            object.items.push(obj(&mut r)?);
        }
        let n = r.count(152, 10_000_000, "object pair count")?;
        for _ in 0..n {
            object.pairs.push([obj(&mut r)?, obj(&mut r)?]);
        }
        let object2 = placed(&mut r, "object2")?;

        // sound
        let mut sound = Sound { bgm_names: names(&mut r, "bgm names")?, ..Default::default() };
        let n = r.count(36, 10_000_000, "bgm zone count")?;
        for _ in 0..n {
            sound.bgm_zones.push(BgmZone {
                min: r.f3("bgm zone")?,
                max: r.f3("bgm zone")?,
                rot_y: r.f32("bgm zone")?,
                bgm_index: r.i32("bgm zone")?,
                flag: r.i32("bgm zone")?,
            });
        }
        sound.wav_names = names(&mut r, "wav names")?;
        let n = r.count(28, 10_000_000, "wav zone count")?;
        for _ in 0..n {
            let min = r.f3("wav zone")?;
            let max = r.f3("wav zone")?;
            let k = r.count(4, 1000, "wav zone entries")?;
            let mut wavs = Vec::with_capacity(k);
            for _ in 0..k {
                wavs.push(r.i32("wav index")?);
            }
            sound.wav_zones.push(WavZone { min, max, wavs });
        }
        let n = r.count(20, 10_000_000, "sound point count")?;
        for _ in 0..n {
            sound.points.push(SoundPoint { wav: r.i32("sound point")?, pos: r.f3("sound point")?, radius: r.f32("sound point")? });
        }

        // zones
        let n = r.count(28, 10_000_000, "oriented box count")?;
        let mut boxes = Vec::with_capacity(n);
        for _ in 0..n {
            boxes.push(OrientedBox { min: r.f3("box")?, max: r.f3("box")?, rot_y: r.f32("box")? });
        }
        let n = r.count(556, 1_000_000, "portal count")?;
        let mut portals = Vec::with_capacity(n);
        for _ in 0..n {
            portals.push(WldPortal {
                min: r.f3("portal")?,
                max: r.f3("portal")?,
                rot_y: r.f32("portal")?,
                name: r.name256("portal name")?,
                dest_name: r.name_n(248, "portal destination name")?,
                country: r.i32("portal country")?,
                level_min: r.i16("portal level")?,
                level_max: r.i16("portal level")?,
                dest_map: r.i32("portal destination map")?,
                dest_pos: r.f3("portal destination")?,
            });
        }
        // zone groups: the client reads groups until the records read reach `total`; an empty group would loop forever
        let total = r.count(0, 10_000_000, "zone record count")?;
        let mut zone_groups = Vec::new();
        let mut seen = 0usize;
        while seen < total {
            let k = r.count(36, 100_000, "zone group size")?;
            if k == 0 {
                return Err(r.fail("empty zone group (the client would loop forever)"));
            }
            let mut g = ZoneGroup::default();
            for _ in 0..k {
                g.boxes.push(ZoneBox {
                    min: r.f3("zone box")?,
                    max: r.f3("zone box")?,
                    rot_y: r.f32("zone box")?,
                    country: r.i32("zone country")?,
                    attr: r.i32("zone attr")?,
                });
            }
            seen += k;
            zone_groups.push(g);
        }
        let n = r.count(548, 1_000_000, "area count")?;
        let mut areas = Vec::with_capacity(n);
        for _ in 0..n {
            areas.push(WldArea {
                min: r.f3("area")?,
                max: r.f3("area")?,
                rot_y: r.f32("area")?,
                name: r.name256("area name")?,
                tga: r.name256("area caption")?,
                mode: r.i32("area mode")?,
                extra: r.i32("area extra")?,
            });
        }
        // NPC list: declared count n; after each record the client subtracts its extra point count from n (FUN_005254e0)
        let declared = r.count(0, 10_000_000, "npc count")?;
        let mut left = declared as i64;
        let mut npcs = Vec::new();
        while (npcs.len() as i64) < left {
            let kind = r.i32("npc type")?;
            let type_id = r.i32("npc type id")?;
            let pos = r.f3("npc position")?;
            let angle = r.f32("npc angle")?;
            let k = r.count(12, 100_000, "npc extra points")?;
            let mut extra = Vec::with_capacity(k);
            for _ in 0..k {
                extra.push(r.f3("npc path point")?);
            }
            left -= k as i64;
            npcs.push(WldNpc { kind, type_id, pos, angle, extra });
        }

        let mut sky = Vec::new();
        if kind == WldKind::Field {
            for _ in 0..3 {
                sky.push(r.name256("sky name")?);
            }
        }
        let mut colours = [[0.0f32; 3]; 3];
        for c in colours.iter_mut() {
            *c = r.f3("colour")?;
        }
        let fog_near = r.f32("fog near")?;
        let fog_far = r.f32("fog far")?;
        r.end("map")?;

        Ok(Wld {
            kind,
            terrain,
            textures,
            water,
            dg,
            building,
            shape,
            tree,
            grass,
            vani,
            vani2,
            dungeon,
            mani,
            effect_file,
            effects,
            object,
            object2,
            sound,
            boxes,
            portals,
            zone_groups,
            areas,
            npc_declared: declared as u32,
            npcs,
            sky,
            colours,
            fog_near,
            fog_far,
        })
    }
}

fn names(r: &mut Cur, what: &str) -> Result<Vec<String>> {
    let n = r.count(256, 100_000, what)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(r.name256(what)?);
    }
    Ok(v)
}

fn placed(r: &mut Cur, what: &str) -> Result<PlacedList> {
    let names = names(r, what)?;
    let n = r.count(40, 10_000_000, what)?;
    let mut items = Vec::with_capacity(n);
    for _ in 0..n {
        items.push(Placement { index: r.i32(what)?, pos: r.f3(what)?, dir: r.f3(what)?, up: r.f3(what)? });
    }
    Ok(PlacedList { names, items })
}

fn obj(r: &mut Cur) -> Result<Obj> {
    let index = r.i32("object index")?;
    let pos = r.f3("object")?;
    let dir = r.f3("object")?;
    let up = r.f3("object")?;
    let mut params = [0u32; 9];
    for p in params.iter_mut() {
        *p = r.u32("object parameter")?;
    }
    Ok(Obj { index, pos, dir, up, params })
}
