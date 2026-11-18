//! JSON documents: one per map, floats as JSON numbers (shortest decimal that parses back to the same f32), blocked cells
//! as base64 of the packed bytes.

use std::fmt;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

use crate::bitgrid::BitGrid;
use crate::model::*;
use crate::validate::non_finite_records;

#[derive(Debug)]
pub enum JsonError {
    /// The map holds a NaN or infinity, which JSON cannot carry.
    NonFinite { section: &'static str, index: usize },
    Parse(serde_json::Error),
    /// `blocked` is not valid base64.
    BadBase64(String),
    /// The decoded `blocked` bytes do not match `ceil(size^2 / 8)`.
    BadBlockedLength { size: u32, bytes: usize },
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonError::NonFinite { section, index } => {
                write!(f, "{section} record {index} holds NaN or infinity, which JSON cannot carry")
            }
            JsonError::Parse(e) => write!(f, "JSON error: {e}"),
            JsonError::BadBase64(e) => write!(f, "blocked: not valid base64 ({e})"),
            JsonError::BadBlockedLength { size, bytes } => {
                write!(f, "blocked: {bytes} bytes decoded, a {size}x{size} map needs ceil({size}^2 / 8)")
            }
        }
    }
}

impl std::error::Error for JsonError {}

#[derive(Serialize, Deserialize)]
struct Doc {
    size: u32,
    /// base64 (standard alphabet, padded) of the packed bytes, bit `z * size + x`, LSB first.
    blocked: String,
    section_a: Vec<RawA>,
    points_b: Vec<[f32; 3]>,
    mob_areas: Vec<MobArea>,
    npcs: Vec<NpcEntry>,
    portals: Vec<Portal>,
    binds: Vec<Bind>,
    regions: Vec<Region>,
}

fn to_doc(m: &Svmap) -> Result<Doc, JsonError> {
    if let Some((s, index)) = non_finite_records(m).first() {
        return Err(JsonError::NonFinite { section: s.label(), index: *index });
    }
    Ok(Doc {
        size: m.size,
        blocked: STANDARD.encode(m.blocked.as_bytes()),
        section_a: m.section_a.clone(),
        points_b: m.points_b.clone(),
        mob_areas: m.mob_areas.clone(),
        npcs: m.npcs.clone(),
        portals: m.portals.clone(),
        binds: m.binds.clone(),
        regions: m.regions.clone(),
    })
}

/// Compact JSON.
pub fn to_json(m: &Svmap) -> Result<String, JsonError> {
    serde_json::to_string(&to_doc(m)?).map_err(JsonError::Parse)
}

/// Indented JSON.
pub fn to_json_pretty(m: &Svmap) -> Result<String, JsonError> {
    serde_json::to_string_pretty(&to_doc(m)?).map_err(JsonError::Parse)
}

pub fn from_json(s: &str) -> Result<Svmap, JsonError> {
    let d: Doc = serde_json::from_str(s).map_err(JsonError::Parse)?;
    let bytes = STANDARD.decode(d.blocked.as_bytes()).map_err(|e| JsonError::BadBase64(e.to_string()))?;
    let n = bytes.len();
    let blocked = BitGrid::from_bytes(d.size, bytes).ok_or(JsonError::BadBlockedLength { size: d.size, bytes: n })?;
    Ok(Svmap {
        size: d.size,
        blocked,
        section_a: d.section_a,
        points_b: d.points_b,
        mob_areas: d.mob_areas,
        npcs: d.npcs,
        portals: d.portals,
        binds: d.binds,
        regions: d.regions,
    })
}
