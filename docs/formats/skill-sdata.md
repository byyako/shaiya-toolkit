# `Skill.SData`

`Skill.SData` is the client's skill table (`Character/Skill.SData` in the archive). The file is [SEED-wrapped](sdata-wrapper.md) in most clients; this page is about the plaintext. The monster skill table (`Monster/NpcSkill.SData`) has the same layout. Shaiya Toolkit's SData tab and [`shaiya sdata table`](../tools/shaiya.md#shaiya-sdata) show it as a table. Both are read only.

## Layout as read

All numbers are little-endian. Strings are a `u32` byte length followed by that many bytes (code page 949 on the original client).

| Part | Type |
|---|---|
| entry count | `u32` (the client keeps only the low 16 bits) |
| per entry: a fixed number of level records | 3, 9 or 15 (see below), always that many |
| per record: name | string |
| per record: description | string |
| per record: fixed block | 116 bytes (3 and 9 levels) or 138 bytes (15 levels) |

The skill number is the entry's position counted from 1; the table view shows it as the `skill` column. The file must end exactly after the last record. A layout is used for a file only when it reads the whole file this way; a file that does not read is shown as hex.

| Variant | Levels per entry | Block |
|---|---|---|
| `skill-3` | 3 | 116 bytes |
| `skill-9` | 9 | 116 bytes |
| `skill-15` | 15 | 138 bytes |

The first field of the block is the level (`u8`, shown as `level`). No other field has a confirmed meaning, so the rest are shown as `f<offset>` with their width (`u8` or `u16`). The 138-byte block is 22 bytes longer than the 116-byte one.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-sdata` | reads the table with the layout that fits the file |
| `shaiya sdata table` and the SData tab's View table | rows and columns, filter, CSV, JSON |

The file is only read.

## Verified on

`skill-3` was derived from the client's own table loader and checked against the whole file on US EP4.5 (patch 108, 326 entries, 978 records), ru-ep3 (882 records) and ru-ep45 (978). `skill-9` was derived and checked the same way on fr-ep54 (372 entries, 3,348 records) and `skill-15` on phoenix-ep63 (451 entries, 6,765 records). The monster skill table (`NpcSkill`) was only measured: the same variant reads its file on each of these clients (the `auto` row of the version matrix counts it as a skill table), but its loader was not read and no field meaning is claimed for it. The numbers are the measured rows of the [version matrix](../version-matrix.md).

## Other episodes

The number of levels is not assumed: `auto` tries the three variants and uses the one that reads the file to its last byte. `skill-15` also reads the files of kr-ep8 (8,775 records) and fawkes-v68 (8,460), and `skill-3` the file of cn-ep4 (978): FIT-ONLY, fits of the data that were not checked against those clients' loaders (see [FIT-ONLY layouts](#fit-only-layouts)). `skill-9` reads the `NpcSkill` file of kr-ep8 the same way.

## FIT-ONLY layouts

The layout files list, in their `evidence` object, the clients whose loader they were read from and the clients whose files they only read to the last byte. On kr-ep8, fawkes-v68 and cn-ep4 the Skill layouts above are FIT-ONLY: nobody read those clients' own loaders (their `game.exe` is packed). The table view and `shaiya sdata table` say so, the JSON has `"status"`, `"loader_verified_on"` and `"fit_only_on"`, and the [version matrix](../version-matrix.md) cell says `FIT-ONLY` with the record count. The column names are those of the verified clients; whether those clients' loaders read the same fields is not checked.

**Upgrade path.** A client moves from `fit_only` to `loader` in the layout file, and its matrix cell from FIT-ONLY to OK, when the layout is read from that client's own loader (an unpacked `game.exe` build or a memory dump) and still reads every Skill file of that client to the last byte.
