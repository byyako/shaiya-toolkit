//! Which texture a model uses. A `.3DC` mesh names none (see [`crate::modellist`]), so the pairs come from the list files: `.MLT`
//! (character parts), `.MON` (monsters, NPCs, vehicles, strip NPCs) and `.ITM` (item `.3DO` models). The index is built once from a list
//! of archive paths and a way to read a file, and answers "the textures of this mesh" in record order (the first is the default).
//!
//! Resolution: the name is looked for in the list file's own folder first (`<folder>/3DC`, `<folder>/DDS` or `<folder>/3DO`), then by
//! bare name in the other folders of the same kind (first by sorted path; reported [`Found::Elsewhere`]); otherwise it is
//! [`Found::Missing`]. A name is never guessed. Letter case never matters. Nothing here writes a file.

use std::collections::HashMap;

use crate::modellist::{name_text, Itm, ListKind, Mlt, Mon};

/// What the alpha channel of a texture is for, as the list file says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaUse {
    /// `.MLT` flag 0: the alpha channel is transparency.
    Transparency,
    /// `.MLT` flag 1: the alpha channel is a glow (inferred; the colour is drawn without it).
    Glow,
    /// `.MON` and `.ITM` say nothing about it (and any other `.MLT` flag).
    Unknown,
}

/// How a name was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// In the list file's own folder.
    OwnFolder,
    /// By bare name in another folder of the same kind.
    Elsewhere,
    /// Nowhere.
    Missing,
}

/// One texture of a mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct TexRef {
    /// The archive path as found; the name as the list gives it when [`TexRef::found`] is false.
    pub texture: String,
    pub found: bool,
    pub alpha: AlphaUse,
    /// The archive path of the list file that names the pair.
    pub list: String,
    /// The record of that list (for a `.MON`, the monster record; the part is not kept).
    pub record: usize,
    pub by: Found,
}

/// What [`TextureIndex::build`] read. `own`, `elsewhere` and `missing` count textures; the `mesh_` fields count the mesh names the
/// same way (a mesh that is missing has no entry in the index, so no texture of it can be asked for).
#[derive(Clone, Debug, Default)]
pub struct IndexReport {
    pub lists_read: usize,
    /// `(path, reason)` of every list file that was not read.
    pub lists_refused: Vec<(String, String)>,
    /// Pairs kept (a record whose index points outside its list is skipped and not counted).
    pub records: usize,
    pub own: usize,
    pub elsewhere: usize,
    pub missing: usize,
    pub mesh_own: usize,
    pub mesh_elsewhere: usize,
    pub mesh_missing: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Folder {
    Mesh3dc,
    Mesh3do,
    Dds,
}

fn norm(p: &str) -> String {
    p.replace('\\', "/")
}

fn lower(p: &str) -> String {
    norm(p).to_lowercase()
}

fn parent(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

fn file_name(p: &str) -> &str {
    p.rsplit_once('/').map_or(p, |(_, f)| f)
}

/// The texture index of one client. Cheap to ask, built once.
#[derive(Clone, Debug, Default)]
pub struct TextureIndex {
    /// Lower-cased mesh archive path to its textures, in record order.
    meshes: HashMap<String, Vec<TexRef>>,
    /// Lower-cased path to the path as the archive spells it (every path the lookups can need).
    files: HashMap<String, String>,
    /// Lower-cased bare name to the archive paths (sorted) in folders of that kind.
    bare: HashMap<(Folder, String), Vec<String>>,
}

/// Which list kind a path is, by its place: `Character/<x>/*.MLT`, `Monster|Npc|Vehicle|Strip/*.MON`, `Item/*.ITM`.
fn list_kind(path: &str) -> Option<ListKind> {
    let low = lower(path);
    let parts: Vec<&str> = low.split('/').collect();
    let ext = parts.last()?.rsplit_once('.')?.1;
    match (parts.first().copied()?, parts.len(), ext) {
        ("character", 3, "mlt") => Some(ListKind::Mlt),
        ("monster" | "npc" | "vehicle" | "strip", 2, "mon") => Some(ListKind::Mon),
        ("item", 2, "itm") => Some(ListKind::Itm),
        _ => None,
    }
}

impl TextureIndex {
    /// Reads every list file among `paths` (archive paths, `/` or `\`) with `read` and resolves the names against `paths`. `read`
    /// gives a file's bytes (`None`: unreadable). `cancelled` is asked before each list file; `None` is returned when it says yes.
    /// A list file that does not read is counted in [`IndexReport::lists_refused`] and skipped.
    pub fn build(paths: &[String], read: &mut dyn FnMut(&str) -> Option<Vec<u8>>, cancelled: &mut dyn FnMut() -> bool) -> Option<(TextureIndex, IndexReport)> {
        let mut idx = TextureIndex::default();
        for p in paths {
            let p = norm(p);
            let low = p.to_lowercase();
            let folder = parent(&low).rsplit_once('/').map_or(parent(&low), |(_, f)| f);
            let kind = match folder {
                "3dc" => Some(Folder::Mesh3dc),
                "3do" => Some(Folder::Mesh3do),
                "dds" => Some(Folder::Dds),
                _ => None,
            };
            if let Some(kind) = kind {
                idx.bare.entry((kind, file_name(&low).to_string())).or_default().push(p.clone());
            }
            idx.files.insert(low, p);
        }
        for v in idx.bare.values_mut() {
            v.sort_by_key(|p| p.to_lowercase());
        }
        let mut lists: Vec<(String, ListKind)> = paths.iter().filter_map(|p| list_kind(p).map(|k| (norm(p), k))).collect();
        lists.sort_by_key(|(p, _)| p.to_lowercase());
        let mut report = IndexReport::default();
        for (path, kind) in lists {
            if cancelled() {
                return None;
            }
            let Some(bytes) = read(&path) else {
                report.lists_refused.push((path, "the file could not be read".into()));
                continue;
            };
            let pairs: Result<Vec<Pair>, String> = match kind {
                ListKind::Mlt => Mlt::parse(&bytes, &path).map_err(|e| e.to_string()).map(|m| {
                    m.records
                        .iter()
                        .enumerate()
                        .filter_map(|(i, r)| {
                            let alpha = match r.alpha_flag {
                                0 => AlphaUse::Transparency,
                                1 => AlphaUse::Glow,
                                _ => AlphaUse::Unknown,
                            };
                            Some(Pair { mesh: name_text(m.mesh_name(r)?), texture: name_text(m.texture_name(r)?), alpha, record: i })
                        })
                        .collect()
                }),
                ListKind::Mon => Mon::parse(&bytes, &path).map_err(|e| e.to_string()).map(|m| {
                    m.records.iter().enumerate().flat_map(|(i, r)| r.parts.iter().map(move |p| Pair { mesh: name_text(&p.mesh), texture: name_text(&p.texture), alpha: AlphaUse::Unknown, record: i })).collect()
                }),
                ListKind::Itm => Itm::parse(&bytes, &path).map_err(|e| e.to_string()).map(|m| {
                    m.records
                        .iter()
                        .enumerate()
                        .filter_map(|(i, r)| Some(Pair { mesh: name_text(m.model_name(r)?), texture: name_text(m.texture_name(r)?), alpha: AlphaUse::Unknown, record: i }))
                        .collect()
                }),
            };
            match pairs {
                Ok(pairs) => {
                    report.lists_read += 1;
                    idx.add(&path, kind, pairs, &mut report);
                }
                Err(why) => report.lists_refused.push((path, why)),
            }
        }
        Some((idx, report))
    }

    fn resolve(&self, own_dir: &str, kind: Folder, name: &str) -> (Option<String>, Found) {
        if name.is_empty() {
            return (None, Found::Missing);
        }
        let sub = match kind {
            Folder::Mesh3dc => "3dc",
            Folder::Mesh3do => "3do",
            Folder::Dds => "dds",
        };
        let key = format!("{}/{sub}/{}", own_dir.to_lowercase(), name.to_lowercase());
        if let Some(p) = self.files.get(&key) {
            return (Some(p.clone()), Found::OwnFolder);
        }
        match self.bare.get(&(kind, name.to_lowercase())).and_then(|v| v.first()) {
            Some(p) => (Some(p.clone()), Found::Elsewhere),
            None => (None, Found::Missing),
        }
    }

    fn add(&mut self, list: &str, kind: ListKind, pairs: Vec<Pair>, report: &mut IndexReport) {
        let dir = parent(list).to_string();
        let mesh_kind = if kind == ListKind::Itm { Folder::Mesh3do } else { Folder::Mesh3dc };
        for p in pairs {
            let (mesh, mesh_by) = self.resolve(&dir, mesh_kind, &p.mesh);
            match mesh_by {
                Found::OwnFolder => report.mesh_own += 1,
                Found::Elsewhere => report.mesh_elsewhere += 1,
                Found::Missing => report.mesh_missing += 1,
            }
            let (tex, by) = self.resolve(&dir, Folder::Dds, &p.texture);
            report.records += 1;
            match by {
                Found::OwnFolder => report.own += 1,
                Found::Elsewhere => report.elsewhere += 1,
                Found::Missing => report.missing += 1,
            }
            let Some(mesh) = mesh else { continue };
            let found = tex.is_some();
            let r = TexRef { texture: tex.unwrap_or(p.texture), found, alpha: p.alpha, list: list.to_string(), record: p.record, by };
            self.meshes.entry(mesh.to_lowercase()).or_default().push(r);
        }
    }

    /// The textures of a `.3DC` (`.MLT`, `.MON`) or `.3DO` (`.ITM`) mesh, in record order (the first is the default); empty when no
    /// list names the mesh, in which case it is drawn grey. `mesh_path` is an archive path; case and `\` do not matter.
    pub fn for_mesh(&self, mesh_path: &str) -> &[TexRef] {
        self.meshes.get(&lower(mesh_path)).map_or(&[], |v| v.as_slice())
    }

    /// How many meshes have at least one texture pair.
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// How many meshes have at least one texture that was found.
    pub fn textured_mesh_count(&self) -> usize {
        self.meshes.values().filter(|v| v.iter().any(|t| t.found)).count()
    }

    /// The `.3DE` effect mesh `m3de_path` is drawn with the texture of its own name `name` (the name its file stores) from the `DDS`
    /// folder beside its folder (`Effect/3DE` -> `Effect/DDS`) or `Effect/DDS`; the name is tried as given, as `.dds` and as `.tga`
    /// like [`crate::texture_candidates`]. Returns the archive path as found, or `None`.
    pub fn for_effect_mesh(&self, m3de_path: &str, name: &str) -> Option<String> {
        let path = norm(m3de_path);
        let beside = format!("{}/DDS", parent(parent(&path)));
        let found = [beside.as_str(), "Effect/DDS"].into_iter().find_map(|dir| crate::texture_candidates(dir, name).into_iter().find_map(|c| self.files.get(&c.to_lowercase()).cloned()));
        found
    }
}

struct Pair {
    mesh: String,
    texture: String,
    alpha: AlphaUse,
    record: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::lists::{itm_bytes, mlt_bytes, mon_bytes, ItmRec, MonRec};
    use std::collections::HashMap as Map;

    fn build(files: &[(&str, Vec<u8>)]) -> (TextureIndex, IndexReport) {
        let paths: Vec<String> = files.iter().map(|(p, _)| p.to_string()).collect();
        let map: Map<String, Vec<u8>> = files.iter().map(|(p, b)| (p.to_string(), b.clone())).collect();
        TextureIndex::build(&paths, &mut |p| map.get(p).cloned(), &mut || false).expect("not cancelled")
    }

    fn empty(paths: &[&str]) -> Vec<(&'static str, Vec<u8>)> {
        paths.iter().map(|p| (Box::leak(p.to_string().into_boxed_str()) as &str, vec![0u8])).collect()
    }

    fn mon(name: &'static str, parts: &'static [(&'static str, &'static str)]) -> MonRec<'static> {
        MonRec { name, flag: 0, slots: [""; 17], parts, size: 1.0, attachments: &[] }
    }

    #[test]
    fn a_character_part_gets_its_mlt_textures_in_record_order() {
        let mlt = mlt_bytes(&["a_torso.3DC"], &["a_one.dds", "a_two.dds"], &[(0, 1, 1), (0, 0, 0)]);
        let mut files = empty(&["Character/Human/3DC/a_torso.3DC", "Character/Human/DDS/a_one.dds", "Character/Human/DDS/a_two.dds"]);
        files.push(("Character/Human/h.MLT", mlt));
        let (idx, rep) = build(&files);
        let t = idx.for_mesh("Character/Human/3DC/a_torso.3DC");
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].texture.as_str(), t[0].alpha, t[0].record, t[0].by), ("Character/Human/DDS/a_two.dds", AlphaUse::Glow, 0, Found::OwnFolder));
        assert_eq!((t[1].texture.as_str(), t[1].alpha, t[1].record), ("Character/Human/DDS/a_one.dds", AlphaUse::Transparency, 1));
        assert_eq!(t[0].list, "Character/Human/h.MLT");
        assert!(t.iter().all(|r| r.found));
        assert_eq!((rep.lists_read, rep.records, rep.own, rep.mesh_own), (1, 2, 2, 2));
    }

    #[test]
    fn a_monster_part_resolves_in_its_own_folder_first() {
        let m = mon_bytes(&[mon("Bear", &[("bear.3DC", "bear.dds")])]);
        let mut files = empty(&["Monster/3DC/bear.3DC", "Monster/DDS/bear.dds", "Npc/DDS/bear.dds"]);
        files.push(("Monster/monster.MON", m));
        let (idx, _) = build(&files);
        let t = idx.for_mesh("Monster/3DC/bear.3DC");
        assert_eq!((t.len(), t[0].texture.as_str(), t[0].by, t[0].alpha), (1, "Monster/DDS/bear.dds", Found::OwnFolder, AlphaUse::Unknown));
    }

    #[test]
    fn an_npc_part_found_elsewhere_is_marked_so() {
        let m = mon_bytes(&[mon("Guard", &[("guard.3DC", "guard.dds")])]);
        let mut files = empty(&["Character/Zed/3DC/guard.3DC", "Character/Human/DDS/guard.dds", "Character/Alpha/DDS/guard.dds"]);
        files.push(("Npc/NPC.MON", m));
        let (idx, rep) = build(&files);
        let t = idx.for_mesh("Character/Zed/3DC/guard.3DC");
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].by, t[0].found), (Found::Elsewhere, true));
        assert_eq!(t[0].texture, "Character/Alpha/DDS/guard.dds", "the first by sorted path");
        assert_eq!((rep.mesh_elsewhere, rep.elsewhere, rep.own), (1, 1, 0));
    }

    #[test]
    fn a_missing_texture_is_reported_not_guessed() {
        let m = mon_bytes(&[mon("Bear", &[("bear.3DC", "bear.dds"), ("bear2.3DC", "")])]);
        let mut files = empty(&["Monster/3DC/bear.3DC", "Monster/3DC/bear2.3DC", "Monster/DDS/bear.tga.dds"]);
        files.push(("Monster/monster.MON", m));
        let (idx, rep) = build(&files);
        let t = idx.for_mesh("Monster/3DC/bear.3DC");
        assert_eq!((t[0].found, t[0].by, t[0].texture.as_str()), (false, Found::Missing, "bear.dds"));
        let t = idx.for_mesh("Monster/3DC/bear2.3DC");
        assert_eq!((t[0].found, t[0].by, t[0].texture.as_str()), (false, Found::Missing, ""));
        assert_eq!(rep.missing, 2);
        assert_eq!(idx.for_mesh("Monster/3DC/nobody.3DC"), &[] as &[TexRef], "a mesh no list names has none");
    }

    #[test]
    fn lookups_ignore_letter_case() {
        let m = mon_bytes(&[mon("Ape", &[("MOB_APE.3dc", "Mob_Ape.DDS")])]);
        let mut files = empty(&["monster/3dc/mob_ape.3DC", "MONSTER/dds/MOB_APE.dds"]);
        files.push(("Monster/monster.mon", m));
        let (idx, _) = build(&files);
        for q in ["Monster/3DC/Mob_Ape.3DC", r"MONSTER\3dc\MOB_APE.3DC"] {
            let t = idx.for_mesh(q);
            assert_eq!(t.len(), 1, "{q}");
            assert_eq!((t[0].by, t[0].texture.as_str()), (Found::OwnFolder, "MONSTER/dds/MOB_APE.dds"));
        }
    }

    #[test]
    fn an_item_model_gets_its_itm_textures() {
        let itm = itm_bytes(&["01.3DO"], &["01.dds"], &[ItmRec { words: [0, 0, 2, 0, 0, 0], extra: None }]);
        let mut files = empty(&["Item/3DO/01.3DO", "Item/DDS/01.dds"]);
        files.push(("Item/01.ITM", itm));
        let (idx, rep) = build(&files);
        let t = idx.for_mesh("Item/3DO/01.3DO");
        assert_eq!((t.len(), t[0].texture.as_str(), t[0].alpha, t[0].by), (1, "Item/DDS/01.dds", AlphaUse::Unknown, Found::OwnFolder));
        assert_eq!(rep.records, 1);
    }

    #[test]
    fn a_refused_list_is_counted_and_skipped() {
        let good = mon_bytes(&[mon("Bear", &[("bear.3DC", "bear.dds")])]);
        let mut files = empty(&["Monster/3DC/bear.3DC", "Monster/DDS/bear.dds"]);
        files.push(("Monster/monster.MON", good));
        files.push(("Npc/NPC.MON", b"MO4 not this layout".to_vec()));
        files.push(("Strip/s.MON", Vec::new()));
        let (idx, rep) = build(&files);
        assert_eq!(rep.lists_read, 1);
        let refused: Vec<&str> = rep.lists_refused.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(refused, ["Npc/NPC.MON", "Strip/s.MON"]);
        assert!(rep.lists_refused[0].1.contains("another episode"), "{:?}", rep.lists_refused);
        assert_eq!(idx.for_mesh("Monster/3DC/bear.3DC").len(), 1);
    }

    #[test]
    fn building_can_be_cancelled() {
        let good = mon_bytes(&[mon("Bear", &[("bear.3DC", "bear.dds")])]);
        let paths = vec!["Monster/monster.MON".to_string(), "Npc/NPC.MON".to_string()];
        let mut asked = 0;
        let r = TextureIndex::build(&paths, &mut |_| Some(good.clone()), &mut || {
            asked += 1;
            asked > 1
        });
        assert!(r.is_none());
        assert_eq!(asked, 2, "asked before each list file, stopped at the first yes");
    }

    #[test]
    fn a_list_in_the_wrong_place_or_with_a_record_out_of_range_adds_nothing() {
        let mlt = mlt_bytes(&["a.3DC"], &["a.dds"], &[(0, 5, 0)]);
        let mut files = empty(&["Character/Human/3DC/a.3DC", "Character/Human/DDS/a.dds"]);
        files.push(("Character/Human/h.MLT", mlt.clone()));
        files.push(("Character/h2.MLT", mlt));
        let (idx, rep) = build(&files);
        assert_eq!((rep.lists_read, rep.records, idx.mesh_count()), (1, 0, 0));
    }

    #[test]
    fn an_effect_mesh_finds_its_texture_in_effect_dds() {
        let files = empty(&["Effect/3DE/ring.3DE", "Effect/DDS/ring.dds", "Effect/DDS/flame.dds"]);
        let (idx, _) = build(&files);
        assert_eq!(idx.for_effect_mesh("Effect/3DE/ring.3DE", "RING.dds").as_deref(), Some("Effect/DDS/ring.dds"));
        assert_eq!(idx.for_effect_mesh("Effect/3DE/ring.3DE", "flame.tga").as_deref(), Some("Effect/DDS/flame.dds"));
        assert_eq!(idx.for_effect_mesh("Effect/3DE/ring.3DE", "none.dds"), None);
        assert_eq!(idx.for_effect_mesh("Effect/3DE/ring.3DE", ""), None);
    }
}
