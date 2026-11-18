//! The in-memory model of an `.svmap` file. Layout and meaning: `docs/formats/svmap.md`.

use serde::{Deserialize, Serialize};

use crate::bitgrid::BitGrid;

/// Axis-aligned box, `min`/`max` = (x, y, z).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn centre(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }
}

/// Section A: `{f32 x,y,z, u32, u32}`. Meaning unknown; empty in all 77 original files.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawA {
    pub pos: [f32; 3],
    pub a: u32,
    pub b: u32,
}

/// One monster of a spawn area: monster id (record index in `Monster.SData`) and how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobEntry {
    pub mob: u32,
    pub count: u32,
}

/// Section 4: a spawn box with 0-4 (originals) monster entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MobArea {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub mobs: Vec<MobEntry>,
}

impl MobArea {
    pub fn centre(&self) -> [f32; 3] {
        Aabb { min: self.min, max: self.max }.centre()
    }
}

/// One point of an NPC: where it stands (the first) or a stop on its walk path; `dir` in radians.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NpcPoint {
    pub pos: [f32; 3],
    pub dir: f32,
}

/// Section 5: NPC `kind` (type 0-12 in the originals) and `type_id` (the index in that type's list in `NpcQuest.SData`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NpcEntry {
    pub kind: u32,
    pub type_id: u32,
    pub points: Vec<NpcPoint>,
}

/// Section 6, 36 bytes: where the portal stands, who may use it, where it leads.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Portal {
    pub pos: [f32; 3],
    pub country: u32,
    pub level_min: u16,
    pub level_max: u16,
    pub to_map: u32,
    pub to_pos: [f32; 3],
}

/// Section 7: respawn areas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bind {
    pub country: u32,
    pub attr: u32,
    pub boxes: Vec<Aabb>,
}

/// Section 8: a named area box with raw tail fields (`kind` = the `.wld` area mode).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub kind: u32,
    pub b: u8,
    pub c: u8,
    pub d: u16,
}

impl Region {
    pub fn centre(&self) -> [f32; 3] {
        Aabb { min: self.min, max: self.max }.centre()
    }
}

/// A whole map file.
#[derive(Clone, Debug, PartialEq)]
pub struct Svmap {
    pub size: u32,
    pub blocked: BitGrid,
    pub section_a: Vec<RawA>,
    pub points_b: Vec<[f32; 3]>,
    pub mob_areas: Vec<MobArea>,
    pub npcs: Vec<NpcEntry>,
    pub portals: Vec<Portal>,
    pub binds: Vec<Bind>,
    pub regions: Vec<Region>,
}

impl Svmap {
    /// An empty map of the given side (all cells free, no records).
    pub fn new(size: u32) -> Svmap {
        Svmap {
            size,
            blocked: BitGrid::new(size),
            section_a: Vec::new(),
            points_b: Vec::new(),
            mob_areas: Vec::new(),
            npcs: Vec::new(),
            portals: Vec::new(),
            binds: Vec::new(),
            regions: Vec::new(),
        }
    }
}
