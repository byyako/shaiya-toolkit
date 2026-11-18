//! What this client does not match. The map and rule code was written and checked against the US EP4.5 client; on another episode
//! a table may not read or a name may be missing. [`client_notes`] says so in one sentence per finding, when a client is opened, so
//! the editor can show it on the client chip and `shaiya svmap` can print it. Nothing here refuses anything: a note only tells the
//! user which rules or names are off. The assumptions are listed in `docs/version-matrix.md`.

use std::collections::BTreeSet;

use crate::client_rules::ClientRules;

/// Highest map id the name table is probed for (the table ends far below it).
const NAME_TABLE_PROBE: u32 = 256;

/// The sysmsg ids of the EP4.5 map-name table (one per group of map ids), without repeats.
pub fn map_name_ids() -> BTreeSet<u32> {
    (0..NAME_TABLE_PROBE).filter_map(shaiya_clientdata::map_name_msg).collect()
}

/// One sentence per finding about `rules`' client: unreadable `Monster.SData` or `NpcQuest.SData` (read with the EP4.5 layout) and a
/// `sysmsg-uni.txt` that lacks ids of the EP4.5 map-name table. Empty when everything reads as on EP4.5.
pub fn client_notes(rules: &ClientRules) -> Vec<String> {
    let mut notes = Vec::new();
    if rules.monsters.is_none() {
        notes.push(if rules.source.exists("Monster/Monster.SData") {
            "Monster.SData did not read with the EP4.5 layout: monster names and levels and the monster rules are off.".to_string()
        } else {
            "Monster.SData is missing from this client: monster names and levels and the monster rules are off.".to_string()
        });
    }
    if rules.npcs.is_none() {
        notes.push(if rules.source.exists("Npc/NpcQuest.SData") {
            "NpcQuest.SData did not read with the EP4.5 layout: NPC names and the NPC and gatekeeper rules are off.".to_string()
        } else {
            "NpcQuest.SData is missing from this client: NPC names and the NPC and gatekeeper rules are off.".to_string()
        });
    }
    let ids = map_name_ids();
    let sys = rules.source.sysmsg().ok();
    let lacking = ids.iter().filter(|id| !sys.as_ref().and_then(|s| s.get(**id)).is_some_and(|t| !t.trim().is_empty())).count();
    if lacking > 0 {
        notes.push(format!(
            "Map names and the map-no-name rule come from the EP4.5 client's table; this client's sysmsg lacks {lacking} of its {} ids, so some names and map-no-name findings may be wrong.",
            ids.len()
        ));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{tiny_wld, utf16_bom, TempDir};
    use std::path::Path;

    fn put(dir: &Path, path: &str, bytes: &[u8]) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// An extracted client folder with one map and a sysmsg that holds `ids`.
    fn client(ids: &[u32]) -> TempDir {
        let dir = TempDir::new("episode");
        put(dir.path(), "world/1.wld", &tiny_wld(4).to_bytes());
        if !ids.is_empty() {
            let mut text = String::new();
            for id in ids {
                text.push_str(&format!("{id}\t\t\"name {id}\"\r\n"));
            }
            put(dir.path(), "sysmsg-uni.txt", &utf16_bom(&text));
        }
        dir
    }

    #[test]
    fn the_name_table_has_its_ids() {
        let ids = map_name_ids();
        assert!(ids.contains(&5068) && ids.contains(&5427) && ids.contains(&5417));
        assert_eq!(ids.len(), 47, "one id per group of maps in the table");
    }

    #[test]
    fn missing_tables_and_names_each_give_one_note() {
        // nothing but one map: no Monster, no NpcQuest, no sysmsg
        let dir = client(&[]);
        let rules = ClientRules::open(dir.path()).unwrap();
        let notes = client_notes(&rules);
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[0].starts_with("Monster.SData is missing"), "absent files are missing, not unreadable");
        assert!(notes[1].starts_with("NpcQuest.SData is missing"));
        assert!(notes[2].contains(&format!("lacks {n} of its {n} ids", n = map_name_ids().len())), "{}", notes[2]);

        // junk tables read as unreadable too
        put(dir.path(), "Monster/Monster.SData", b"junk");
        put(dir.path(), "Npc/NpcQuest.SData", b"junk");
        let rules = ClientRules::open(dir.path()).unwrap();
        let junk = client_notes(&rules);
        assert_eq!(junk.len(), 3);
        assert!(junk[0].starts_with("Monster.SData did not read") && junk[1].starts_with("NpcQuest.SData did not read"), "{junk:?}");

        // a sysmsg with some of the ids: the note counts the rest
        let some: Vec<u32> = map_name_ids().into_iter().take(10).collect();
        let dir = client(&some);
        let rules = ClientRules::open(dir.path()).unwrap();
        let notes = client_notes(&rules);
        let want = format!("lacks {} of its {} ids", map_name_ids().len() - 10, map_name_ids().len());
        assert!(notes.iter().any(|n| n.contains(&want)), "{notes:?}");

        // a sysmsg with every id and no SData: only the two table notes
        let all: Vec<u32> = map_name_ids().into_iter().collect();
        let dir = client(&all);
        let mut rules = ClientRules::open(dir.path()).unwrap();
        let notes = client_notes(&rules);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(!notes.iter().any(|n| n.contains("sysmsg")));

        // every table present: no note at all
        rules.monsters = Some(shaiya_clientdata::Monsters { records: Vec::new() });
        assert_eq!(client_notes(&rules).len(), 1, "only the NpcQuest note is left");
        rules.npcs = Some(shaiya_clientdata::NpcQuest { lists: Vec::new(), quests_offset: 0 });
        assert!(client_notes(&rules).is_empty());
    }

    /// An id whose text is blank counts as missing, as the map name lookup does.
    #[test]
    fn a_blank_name_is_missing() {
        let all: Vec<u32> = map_name_ids().into_iter().collect();
        let dir = client(&all);
        let text = utf16_bom(&format!("{}\t\t\" \"\r\n", all[0]));
        put(dir.path(), "sysmsg-uni.txt", &{
            let mut t = String::new();
            for id in &all[1..] {
                t.push_str(&format!("{id}\t\t\"x\"\r\n"));
            }
            let mut b = utf16_bom(&t);
            b.extend_from_slice(&text[2..]);
            b
        });
        let rules = ClientRules::open(dir.path()).unwrap();
        let notes = client_notes(&rules);
        assert!(notes.iter().any(|n| n.contains(&format!("lacks 1 of its {} ids", all.len()))), "{notes:?}");
    }
}
