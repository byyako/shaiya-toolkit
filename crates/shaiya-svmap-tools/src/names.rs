//! Names of things: monsters, NPCs, maps, regions, and the one-line labels the outliner, tooltips and the viewport use.
//! Without client data the labels fall back to ids.

use shaiya_clientdata::sdata::{Monster, Npc};
use shaiya_svmap::{Bind, MobArea, NpcEntry, Portal, Region, SectionId, Svmap};

use crate::assets::MapAssets;
use crate::client_rules::ClientRules;

pub const NPC_KIND_NAMES: [&str; 14] = [
    "placeholder",
    "Merchant",
    "Gatekeeper",
    "Blacksmith",
    "PvP manager",
    "Gambling house",
    "Warehouse",
    "Normal NPC",
    "Guard",
    "Animal",
    "Apprentice",
    "Guild master",
    "Dead NPC",
    "Combat commander",
];

pub fn npc_kind_name(kind: u32) -> String {
    NPC_KIND_NAMES.get(kind as usize).map_or_else(|| format!("type {kind}"), |s| (*s).to_string())
}

/// What the app knows about the client for the current map.
#[derive(Clone, Copy)]
pub struct Lookups<'a> {
    pub rules: Option<&'a ClientRules>,
    pub assets: Option<&'a MapAssets>,
    pub map_id: Option<u32>,
    /// The area names of the tab's own client half (with its unsaved renames); they win over the client's file.
    pub names: Option<&'a shaiya_clientdata::text::AreaNames>,
}

impl<'a> Lookups<'a> {
    #[cfg(test)]
    pub fn none() -> Lookups<'static> {
        Lookups { rules: None, assets: None, map_id: None, names: None }
    }

    pub fn mob(&self, id: u32) -> Option<&'a Monster> {
        self.rules?.monsters.as_ref()?.get(id)
    }

    pub fn npc(&self, kind: u32, type_id: u32) -> Option<&'a Npc> {
        self.rules?.npcs.as_ref()?.get(kind, type_id)
    }

    pub fn npc_list(&self, kind: u32) -> Option<&'a [Npc]> {
        let nq = self.rules?.npcs.as_ref()?;
        nq.lists.get((kind as usize).checked_sub(1)?).map(Vec::as_slice)
    }

    pub fn region_name(&self, index: usize) -> Option<&'a str> {
        match self.names {
            Some(n) => n.get(index),
            None => self.assets?.area_names.get(index),
        }
        .filter(|s| !s.is_empty())
    }

    pub fn ground(&self, x: f32, z: f32) -> f32 {
        self.assets.map_or(0.0, |a| a.ground_at(x, z))
    }

    pub fn mob_name(&self, id: u32) -> String {
        match self.mob(id) {
            Some(m) if !m.name.trim().is_empty() => m.name.trim().to_string(),
            _ => format!("monster {id}"),
        }
    }

    /// Mean `Monster.SData` level of an area's entries (weighted by count), `None` without client data or entries.
    pub fn mean_level(&self, a: &MobArea) -> Option<f32> {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for e in &a.mobs {
            if let Some(m) = self.mob(e.mob) {
                let w = e.count.max(1) as f32;
                sum += m.level as f32 * w;
                n += w;
            }
        }
        if n > 0.0 {
            Some(sum / n)
        } else {
            None
        }
    }

    pub fn mob_area_label(&self, a: &MobArea) -> String {
        if a.mobs.is_empty() {
            return "(no monsters)".to_string();
        }
        let parts: Vec<String> = a.mobs.iter().map(|e| format!("{} x{}", self.mob_name(e.mob), e.count)).collect();
        let mut s = parts.join(", ");
        if let Some(l) = self.mean_level(a) {
            s.push_str(&format!(" · lv {}", l.round() as i32));
        }
        s
    }

    pub fn npc_name(&self, n: &NpcEntry) -> String {
        match self.npc(n.kind, n.type_id) {
            Some(x) if !x.name.trim().is_empty() => x.name.trim().to_string(),
            _ if n.kind == 0 => "placeholder".to_string(),
            _ => format!("id {}", n.type_id),
        }
    }

    pub fn npc_label(&self, n: &NpcEntry) -> String {
        format!("{}: {}", npc_kind_name(n.kind), self.npc_name(n))
    }

    pub fn region_label(&self, i: usize, r: &Region) -> String {
        match self.region_name(i) {
            Some(n) => n.to_string(),
            None => format!("kind {}", r.kind),
        }
    }

    pub fn bind_label(&self, b: &Bind) -> String {
        format!("{} · {} box{}", country_name(b.country), b.boxes.len(), if b.boxes.len() == 1 { "" } else { "es" })
    }
}

pub fn country_name(c: u32) -> String {
    match c {
        0 => "both".to_string(),
        1 => "Light".to_string(),
        2 => "Fury".to_string(),
        3 => "both (3)".to_string(),
        n => format!("country {n}"),
    }
}

pub fn portal_country(c: u32) -> String {
    match c {
        0 => "any faction".to_string(),
        1 => "Light".to_string(),
        2 => "Fury".to_string(),
        n if n >= 1000 => format!("instance {n}"),
        n => format!("country {n}"),
    }
}

/// `map_name(id)` supplies the destination's name.
pub fn portal_label(i: usize, p: &Portal, map_name: &dyn Fn(u32) -> String) -> String {
    let lv = if p.level_min == 0 && p.level_max == 0 { "no level limit".to_string() } else { format!("lv {}-{}", p.level_min, p.level_max) };
    format!("#{i} -> {} ({:.0}, {:.0}) · {} · {lv}", map_name(p.to_map), p.to_pos[0], p.to_pos[2], portal_country(p.country))
}

/// One-line label of any record.
pub fn label(look: &Lookups, m: &Svmap, s: SectionId, i: usize, map_name: &dyn Fn(u32) -> String) -> String {
    match s {
        SectionId::MobAreas => m.mob_areas.get(i).map(|a| look.mob_area_label(a)),
        SectionId::Npcs => m.npcs.get(i).map(|n| look.npc_label(n)),
        SectionId::Portals => m.portals.get(i).map(|p| portal_label(i, p, map_name)),
        SectionId::Binds => m.binds.get(i).map(|b| look.bind_label(b)),
        SectionId::Regions => m.regions.get(i).map(|r| look.region_label(i, r)),
        SectionId::SectionB => m.points_b.get(i).map(|p| format!("({:.1}, {:.1}, {:.1})", p[0], p[1], p[2])),
        SectionId::SectionA => m.section_a.get(i).map(|a| format!("({:.1}, {:.1}, {:.1}) · {} / {}", a.pos[0], a.pos[1], a.pos[2], a.a, a.b)),
        SectionId::Size | SectionId::Blocked => None,
    }
    .unwrap_or_default()
}

/// Colour for a mean monster level: blue at 1 to red at 80.
pub fn level_colour(level: Option<f32>) -> [u8; 3] {
    match level {
        None => [120, 130, 150],
        Some(l) => {
            let t = ((l - 1.0) / 79.0).clamp(0.0, 1.0);
            // blue -> cyan-ish -> yellow -> red
            let stops = [[50.0, 110.0, 230.0], [60.0, 200.0, 170.0], [240.0, 220.0, 60.0], [230.0, 60.0, 50.0]];
            let f = t * 3.0;
            let k = (f.floor() as usize).min(2);
            let u = f - k as f32;
            let c = |i: usize| stops[k][i] + (stops[k + 1][i] - stops[k][i]) * u;
            [c(0) as u8, c(1) as u8, c(2) as u8]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_colours_run_blue_to_red() {
        let lo = level_colour(Some(1.0));
        let hi = level_colour(Some(80.0));
        assert!(lo[2] > lo[0]);
        assert!(hi[0] > hi[2]);
        assert_eq!(level_colour(Some(500.0)), hi);
    }

    #[test]
    fn labels_without_client() {
        let look = Lookups::none();
        let a = MobArea { min: [0.0; 3], max: [1.0; 3], mobs: vec![shaiya_svmap::MobEntry { mob: 7, count: 2 }] };
        assert_eq!(look.mob_area_label(&a), "monster 7 x2");
        let n = NpcEntry { kind: 1, type_id: 3, points: vec![] };
        assert_eq!(look.npc_label(&n), "Merchant: id 3");
    }
}
