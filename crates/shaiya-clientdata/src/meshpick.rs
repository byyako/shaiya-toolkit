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

/// The name an animation and its meshes share: the first word of a character's file name, else the name without its last `_word`.
fn prefix_of(stem: &str, character: bool) -> String {
    let prefix = if character || !stem.contains('_') { stem.split('_').next().unwrap_or(stem) } else { stem.rsplit_once('_').map_or(stem, |(a, _)| a) };
    prefix.to_ascii_lowercase()
}

/// Most `.ANI` files [`animations_for`] offers (and reads the bone count of).
pub const ANIMATIONS_MAX: usize = 64;

/// The animations of the same rig as the `.3DC` at `mesh_path` that have enough bones for it, in name order, at most [`ANIMATIONS_MAX`]:
/// the `.ANI` files of the sibling `ANI` folder whose name prefix is the one [`mesh_candidates_in`] uses for that mesh, and whose bone count
/// (`bones_of`; `None` when the file does not read) is above the highest bone the mesh names. A cloak or a mesh without vertices gets none.
pub fn animations_for(paths: &[String], mesh_path: &str, info: MeshInfo, bones_of: &mut dyn FnMut(&str) -> Option<usize>) -> Vec<String> {
    let segs: Vec<&str> = mesh_path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let (Some((file, folders)), true) = (segs.split_last(), info.skinned) else { return Vec::new() };
    let (Some((last, parents)), Some(highest)) = (folders.split_last(), info.highest_bone) else { return Vec::new() };
    if !last.eq_ignore_ascii_case("3dc") {
        return Vec::new();
    }
    let mesh_stem = split_stem(file).0.to_ascii_lowercase();
    let character = segs.len() == 4 && segs[0].eq_ignore_ascii_case("character");
    let mut folder_key: String = parents.iter().map(|p| format!("{}/", p.to_ascii_lowercase())).collect();
    folder_key.push_str("ani/");
    let mut names: Vec<(String, String)> = Vec::new(); // (lower-case path, archive path)
    for path in paths {
        let lower = path.replace('\\', "/").to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix(&folder_key) else { continue };
        if rest.contains('/') {
            continue;
        }
        let (stem, ext) = split_stem(rest);
        let prefix = prefix_of(stem, character);
        if ext == "ani" && (mesh_stem == prefix || mesh_stem.strip_prefix(&prefix).is_some_and(|t| t.starts_with('_'))) {
            names.push((lower, path.clone()));
        }
    }
    names.sort();
    names.truncate(ANIMATIONS_MAX);
    names.into_iter().filter(|(_, p)| bones_of(p).is_some_and(|n| usize::from(highest) < n)).map(|(_, p)| p).collect()
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
    let prefix = prefix_of(stem, character);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    const SKINNED: MeshInfo = MeshInfo { skinned: true, highest_bone: Some(3) };

    #[test]
    fn animations_for_offers_only_the_rig_that_fits() {
        let all = paths(&[
            "Character/Human/ANI/huwm_002_b.ani",
            "Character/Human/ANI/huwm_001_a.ANI",
            "Character/Human/ANI/huwm_short.ani",
            "Character/Human/ANI/huwm_broken.ani",
            "Character/Human/ANI/huwf_001_a.ani",
            "Character/Human/ANI/huwm_notes.txt",
            "Character/Elf/ANI/huwm_001_a.ani",
            "Character/Human/3DC/huwm_torso_01.3dc",
        ]);
        let mesh = "Character/Human/3DC/huwm_torso_01.3dc";
        let mut asked: Vec<String> = Vec::new();
        let mut bones = |p: &str| {
            asked.push(p.to_string());
            match p {
                "Character/Human/ANI/huwm_short.ani" => Some(3),
                "Character/Human/ANI/huwm_broken.ani" => None,
                _ => Some(4),
            }
        };
        let got = animations_for(&all, mesh, SKINNED, &mut bones);
        assert_eq!(got, paths(&["Character/Human/ANI/huwm_001_a.ANI", "Character/Human/ANI/huwm_002_b.ani"]), "same rig, enough bones, name order");
        assert!(!asked.iter().any(|p| p.contains("huwf") || p.contains("Elf") || p.ends_with(".txt")), "other rigs and other files are never read: {asked:?}");
        // a cloak, a mesh without vertices and a path that is not a .3DC get nothing
        assert!(animations_for(&all, mesh, MeshInfo { skinned: false, highest_bone: Some(0) }, &mut |_| Some(9)).is_empty());
        assert!(animations_for(&all, mesh, MeshInfo { skinned: true, highest_bone: None }, &mut |_| Some(9)).is_empty());
        assert!(animations_for(&all, "Character/Human/ANI/huwm_001_a.ani", SKINNED, &mut |_| Some(9)).is_empty());
    }

    #[test]
    fn animations_for_stops_at_the_limit_and_reads_a_monster_folder() {
        let mut all: Vec<String> = (0..100).map(|i| format!("Monster/ANI/wolf_{i:03}.ani")).collect();
        all.push("Monster/ANI/bear_001.ani".into());
        let mut reads = 0usize;
        let got = animations_for(&all, "Monster/3DC/wolf.3dc", SKINNED, &mut |_| {
            reads += 1;
            Some(10)
        });
        assert_eq!((got.len(), reads), (ANIMATIONS_MAX, ANIMATIONS_MAX));
        assert_eq!(got[0], "Monster/ANI/wolf_000.ani");
        assert!(got.iter().all(|p| p.contains("wolf")));
    }
}
