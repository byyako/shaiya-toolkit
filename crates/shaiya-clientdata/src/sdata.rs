//! EP4 `Monster.SData` and `NpcQuest.SData` readers (layouts of Parsec rev e5cbc6d, as in `sdata-viewer/Core.cs`). Only the
//! parts the editor needs are kept; both parsers read every field of the part they cover.

use crate::error::{Cur, Result};

/// One record of `Monster.SData`. The id used by svmap monster areas is the zero-based index in [`Monsters::records`].
#[derive(Clone, Debug, PartialEq)]
pub struct Monster {
    pub name: String,
    pub model: i16,
    pub level: i16,
    pub ai: u8,
    pub hp: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Monsters {
    pub records: Vec<Monster>,
}

impl Monsters {
    /// `data` is the plaintext (see `seed::decrypt`).
    pub fn parse(data: &[u8], file: &str) -> Result<Monsters> {
        let mut r = Cur::new(data, file);
        let n = r.count(4 + 31, 65_535, "monster count")?;
        let mut records = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.str_i32("monster name")?;
            let model = r.i16("model")?;
            let level = r.i16("level")?;
            let ai = r.u8("ai")?;
            let hp = r.i32("hp")?;
            // day, size, element, normal time (i32) + step, chase time (i32) + step, 3 x (attack type, animation),
            // attack plus, quest item (i16)
            r.skip(1 + 1 + 1 + 4 + 1 + 4 + 1 + 6 + 1 + 2, "monster tail")?;
            records.push(Monster { name, model, level, ai, hp });
        }
        r.end("Monster.SData")?;
        Ok(Monsters { records })
    }

    pub fn get(&self, id: u32) -> Option<&Monster> {
        self.records.get(id as usize)
    }
}

/// One NPC of `NpcQuest.SData`, section `kind` (1 merchants ... 13 combat commanders).
#[derive(Clone, Debug, PartialEq)]
pub struct Npc {
    pub kind: u8,
    /// The record's own NpcTypeId field.
    pub type_id: i16,
    pub model: i32,
    pub move_distance: i32,
    pub move_speed: i32,
    pub faction: i32,
    pub name: String,
    pub welcome: String,
}

pub const NPC_SECTIONS: [&str; 13] = [
    "Merchants",
    "Gatekeepers",
    "Blacksmiths",
    "PvP managers",
    "Gambling houses",
    "Warehouses",
    "Normal NPCs",
    "Guards",
    "Animals",
    "Apprentices",
    "Guild masters",
    "Dead NPCs",
    "Combat commanders",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NpcQuest {
    /// `lists[kind - 1]` = the records of NPC type `kind`, in file order.
    pub lists: Vec<Vec<Npc>>,
    /// Byte offset in the plaintext where the quest table starts (after the per-NPC quest links).
    pub quests_offset: usize,
}

fn id_list(r: &mut Cur, what: &str) -> Result<()> {
    let n = r.count(2, 65_536, what)?;
    r.skip(n * 2, what)
}

impl NpcQuest {
    /// Reads the 13 NPC lists and the 256 x 256 quest-link table; the quests themselves are not parsed.
    pub fn parse(data: &[u8], file: &str) -> Result<NpcQuest> {
        let mut r = Cur::new(data, file);
        let mut lists = Vec::new();
        for kind in 1..=13u8 {
            let n = r.count(20, 65_535, "npc count")?;
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let _npc_type = r.u8("npc type")?;
                let type_id = r.i16("npc type id")?;
                if kind == 1 {
                    r.u8("merchant type")?;
                }
                let model = r.i32("model")?;
                let move_distance = r.i32("move distance")?;
                let move_speed = r.i32("move speed")?;
                let faction = r.i32("faction")?;
                let name = r.str_i32("npc name")?;
                let welcome = r.str_i32("npc welcome message")?;
                if kind == 1 {
                    let m = r.count(2, 65_535, "stock count")?;
                    r.skip(m * 2, "stock")?;
                }
                if kind == 2 {
                    for _ in 0..3 {
                        r.skip(2 + 12, "gate")?;
                        r.str_i32("gate target")?;
                        r.skip(4, "gate cost")?;
                    }
                }
                id_list(&mut r, "in-quest list")?;
                id_list(&mut r, "out-quest list")?;
                list.push(Npc { kind, type_id, model, move_distance, move_speed, faction, name, welcome });
            }
            lists.push(list);
        }
        for _ in 0..256 * 256 {
            id_list(&mut r, "start-quest list")?;
            id_list(&mut r, "end-quest list")?;
        }
        Ok(NpcQuest { lists, quests_offset: r.p })
    }

    /// The NPC the server means by (type, type id): the `type_id`-th record of the type's list (1-based, as the
    /// client's `CNpcData::GetNpc` indexes it).
    pub fn get(&self, kind: u32, type_id: u32) -> Option<&Npc> {
        let list = self.lists.get((kind as usize).checked_sub(1)?)?;
        list.get((type_id as usize).checked_sub(1)?)
    }
}
