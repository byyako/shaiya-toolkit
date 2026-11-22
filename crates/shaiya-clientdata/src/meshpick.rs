//! Which `.3DC` meshes suit an animation, by names and two facts per mesh (no list file is read, nothing is drawn). The toolkit's
//! Archive tab and the version matrix both use it.

use crate::mesh3dc::{C3dc, C3dcLayout};

/// Most `.3DC` files of the sibling folder that are parsed to find the meshes of one animation.
const CANDIDATES_MAX: usize = 64;
/// Meshes drawn together when there are no named default parts.
const DEFAULT_MESHES_MAX: usize = 8;
/// The part words of a character's default parts, in this order.
const PART_WORDS: [&str; 6] = ["torso", "lower", "hand", "boots", "face", "hair"];

fn split_stem(name: &str) -> (&str, String) {
    match name.rsplit_once('.') {
        Some((s, e)) => (s, e.to_ascii_lowercase()),
        None => (name, String::new()),
    }
}

/// What [`mesh_candidates_in`] needs to know about one `.3DC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshInfo {
    /// Not a cloak (a skinned layout).
    pub skinned: bool,
    /// The highest bone index any vertex of any mesh names; `None` when there is no vertex.
    pub highest_bone: Option<u8>,
}

/// [`mesh_candidates`] over a list of archive paths (the `.3DC` ones are used) and a function that reads what is needed about one
/// file (`None` when it does not read). A caller that asks for many animations builds the list once and caches the answers.
pub fn mesh_candidates_in(paths: &[String], ani_path: &str, bone_count: usize, info: &mut dyn FnMut(&str) -> Option<MeshInfo>) -> (Vec<String>, Vec<String>) {
    let none = (Vec::new(), Vec::new());
    let segs: Vec<&str> = ani_path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let Some((file, folders)) = segs.split_last() else { return none };
    let Some((last, parents)) = folders.split_last() else { return none };
    if !last.eq_ignore_ascii_case("ani") {
        return none;
    }
    let stem = split_stem(file).0;
    let character = segs.len() == 4 && segs[0].eq_ignore_ascii_case("character");
    let prefix = if character || !stem.contains('_') { stem.split('_').next().unwrap_or(stem) } else { stem.rsplit_once('_').map_or(stem, |(a, _)| a) };
    let prefix = prefix.to_ascii_lowercase();
    let mut folder_key: String = parents.iter().map(|p| format!("{}/", p.to_ascii_lowercase())).collect();
    folder_key.push_str("3dc/");
    let mut names: Vec<(String, String)> = Vec::new(); // (lower-case stem, archive path)
    for path in paths {
        let lower = path.replace('\\', "/").to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix(&folder_key) else { continue };
        if rest.contains('/') {
            continue;
        }
        let (stem, ext) = split_stem(rest);
        if ext == "3dc" && (stem == prefix || stem.strip_prefix(&prefix).is_some_and(|t| t.starts_with('_'))) {
            names.push((stem.to_string(), path.clone()));
        }
    }
    names.sort();
    names.truncate(CANDIDATES_MAX);
    let mut compatible: Vec<(String, String)> = Vec::new();
    for (stem, path) in names {
        let Some(m) = info(&path) else { continue };
        if m.skinned && m.highest_bone.is_some_and(|b| (b as usize) < bone_count) {
            compatible.push((stem, path));
        }
    }
    let mut defaults: Vec<String> = Vec::new();
    if character {
        for word in PART_WORDS {
            let hit = compatible.iter().find(|(stem, _)| stem.strip_prefix(&prefix).and_then(|t| t.strip_prefix('_')).is_some_and(|t| t.starts_with(word)));
            if let Some((_, p)) = hit {
                defaults.push(p.clone());
            }
        }
    }
    if defaults.is_empty() {
        defaults = compatible.iter().take(DEFAULT_MESHES_MAX).map(|(_, p)| p.clone()).collect();
    }
    (compatible.into_iter().map(|(_, p)| p).collect(), defaults)
}

impl MeshInfo {
    /// The two facts about a parsed `.3DC`.
    pub fn of(c: &C3dc) -> MeshInfo {
        MeshInfo { skinned: c.layout != C3dcLayout::Cloak, highest_bone: c.meshes.iter().flat_map(|m| m.vertices.iter().map(|v| v.bone1.max(v.bone2))).max() }
    }
}
