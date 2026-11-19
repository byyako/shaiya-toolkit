# `Monster.SData`

`Monster.SData` is the client's monster table (`Monster/Monster.SData` in the archive). The file is [SEED-wrapped](sdata-wrapper.md); this page is about the plaintext. The server keeps its monsters in a database, so there is no server copy to read.

## Layout as read

`i32 count`, then count records. Strings are `i32 length` and that many bytes (cp949, NUL-terminated).

| Field | Type |
|---|---|
| name | string |
| model | `i16` |
| level | `i16` |
| ai | `u8` |
| hp | `i32` |
| the rest of the record | 22 bytes, skipped (day, size, element, timers, attack types and animations, attack plus, quest item) |

The file must end exactly after the last record.

## Monster ids

The id used by a monster area in a `.svmap` file ([server map](svmap.md)) is the zero-based record index. Id 0 is the placeholder record "Error Monster".

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `Monsters` | reads name, model, level, ai and hp |
| Shaiya SVMap Editor and `shaiya svmap` | monster names and levels, the colours by level, the heatmap, the Spawns tab, and the rules `mob-unknown` and `area-mob-zero` |
| `shaiya-sdata`, `shaiya sdata table` and the SData tab's View table | every field of the 31-byte block as a column (layout `monster-31`); the names are those above where known, `f<offset>` otherwise; a stored name's trailing NUL is not shown |

The file is only read.

## Verified on

US EP4.5 (2,474 records). The version matrix shows `Monsters::parse` reading it on ru-ep3, ru-ep45, fr-ep54 and phoenix-ep63. The table layout `monster-31` (the record is a name and a block of 31 bytes) was derived from each client's own table loader and checked against the whole file on US EP4.5, ru-ep3 (2,372 records), ru-ep45 (2,474), fr-ep54 (2,992) and phoenix-ep63 (4,511).

## Other episodes

The layout is the original client generation's. It fails on kr-ep8, fawkes-v68 and cn-ep4 in the version matrix. When it does not read, the editor says so in one note, and the monster names, levels and rules are off.
