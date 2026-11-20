# `NpcQuest.SData`

`NpcQuest.SData` holds the NPC tables and the quest data. The client has one copy (`Npc/NpcQuest.SData` in the archive) and the server has another with the same layout and different text: the server's has more NPCs (1,379 against 1,271 on the original files), and the languages differ. The file is [SEED-wrapped](sdata-wrapper.md); everything below is about the plaintext. Strings are `i32 length` followed by that many bytes (cp949, NUL-terminated).

## Layout as read

The file starts with 13 NPC sections, one per NPC type, in this order:

| Type | Section |
|---|---|
| 1 | Merchants |
| 2 | Gatekeepers |
| 3 | Blacksmiths |
| 4 | PvP managers |
| 5 | Gambling houses |
| 6 | Warehouses |
| 7 | Normal NPCs |
| 8 | Guards |
| 9 | Animals |
| 10 | Apprentices |
| 11 | Guild masters |
| 12 | Dead NPCs |
| 13 | Combat commanders |

Each section is `i32 count` followed by the records. A record is:

| Field | Type |
|---|---|
| NPC type | `u8` |
| type id (the record's own id field) | `i16` |
| merchant kind (type 1 only) | `u8` |
| model | `i32` |
| move distance | `i32` |
| move speed | `i32` |
| faction | `i32` |
| name | string |
| welcome message | string |
| stock (type 1 only) | `i32 count`, then count x `i16` |
| **gates** (type 2 only) | 3 x {`u16 map`, `f32 x, y, z`, string name, `u32 cost`} |
| in-quest list | `i32 count`, then count x `u16` |
| out-quest list | `i32 count`, then count x `u16` |

After the 13 sections come 256 x 256 pairs of quest links (a start list and an end list, each `i32 count` and that many `u16`). The quests themselves are not parsed.

## Which record an svmap NPC means

A map NPC (`type`, `type id`) means the `type id`-th record of that type's list, counted from 1. It is an index, not a comparison with the id field.

## Gatekeeper gates

Every gatekeeper (type 2) has exactly three destinations: a map id, a position, the name shown and the cost in gold. The rule `gate-dest-invalid` reports a gate that sends players to a map that does not exist or to a position outside it, and `npc-client-server-differ` compares the gates of the client and server copies.

## Quest records and the table view

The part after the link table is `u32 count` and that many quest records, which the readers above skip. The table view (`shaiya sdata table`, the SData tab's View table) reads them, and the NPC sections, as five tables: `merchants`, `gatekeepers`, `npcs` (types 3 to 13, with a `kind` column), `quest_links` (the 256 x 256 cells) and `quests` (the table shown first). There are two quest record shapes, told apart by whether a layout reads the whole file:

| Variant | A quest record |
|---|---|
| `npcquest-165` | `u16`, two strings, a fixed block of 165 bytes, seven strings |
| `npcquest-87` | `u16`, two strings, a fixed block of 87 bytes, six sub-blocks of 33 bytes each followed by a string, four strings |
| `npcquest-287` (FIT-ONLY) | a fixed block of 287 bytes and no strings; the NPC and gate records also hold no texts |

Quests are numbered from 1 (the `id` column). Fields of the fixed blocks whose meaning is not known are shown as `f<offset>`. The NPC sections are the same in the two verified variants; `npcquest-287` has its own NPC sections without texts.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `NpcQuest` | reads the 13 lists and the link table. It keeps type, id, model, move distance and speed, faction, name, welcome text and the gates. |
| Shaiya SVMap Editor and `shaiya svmap check` | NPC names, `npc-unknown`, `npc-unknown-server`, `npc-client-server-differ` and `gate-dest-invalid` |

The file is only read.

## Verified on

US EP4.5. The version matrix shows `NpcQuest::parse` reading it on ru-ep3, ru-ep45, fr-ep54, phoenix-ep63 and cn-ep4, with these gatekeeper counts: us-108 61, ru-ep3 61, ru-ep45 72, fr-ep54 72, phoenix-ep63 106 and cn-ep4 61.

## Other episodes

The layout is the original client generation's; later episodes change it. `NpcQuest::parse` fails on kr-ep8 and fawkes-v68, so the gates were not measured on those two clients. The editor then turns off the NPC and gatekeeper rules with one note. The text code page differs between episodes. The table view reads kr-ep8 (3,820 quests) and fawkes-v68 (4,182) with the FIT-ONLY layout `npcquest-287`, and cn-ep4 (1,531 quests) with `npcquest-165`, also FIT-ONLY there (see below).

## FIT-ONLY layouts

`npcquest-287` was not read from any client's own loader (the `game.exe` of kr-ep8 and fawkes-v68 is packed): it reads those two files to the last byte, so it is FIT-ONLY. In it the merchant, gatekeeper and NPC records and the gate items have no texts, and a quest is a fixed block of 287 bytes (the verified `npcquest-87` record without its strings). The columns are named by position (`f<offset>`, `l0`, `l1` for lists), only multi-byte widths that aligned with the verified client's values are wider than a byte, and values may be split wrongly. `npcquest-165` on cn-ep4 is a layout verified on other clients that reads that client's file: FIT-ONLY there. The table view, `shaiya sdata table` (stderr) and the JSON (`"status"`, `"loader_verified_on"`, `"fit_only_on"`) say so, and the [version matrix](../version-matrix.md) shows `FIT-ONLY` with the quest count.

**Upgrade path.** A client moves from `fit_only` to `loader` in the layout file, and its matrix cell becomes OK, when the layout is read from that client's own loader (an unpacked `game.exe` build or a memory dump) and still reads every NpcQuest file of that client to the last byte; confirmed names then replace the positions.
