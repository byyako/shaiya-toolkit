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
| `shaiya-sdata`, `shaiya sdata table` and the SData tab's View table | every field of the 31-byte block as a column (layout `monster-31`), or of the 36-byte block of the FIT-ONLY layout `monster-36`; the 31-byte layout names its columns, `monster-36` names none: its columns are `id` (the record's number, counted by the table's loop, not stored in the file), `s0` (the text) and `f<offset>` (the fixed block); a stored name's trailing NUL is not shown |

The file is only read.

## Verified on

US EP4.5 (2,474 records). The version matrix shows `Monsters::parse` reading it on ru-ep3, ru-ep45, fr-ep54 and phoenix-ep63. The table layout `monster-31` (the record is a name and a block of 31 bytes) was derived from each client's own table loader and checked against the whole file on US EP4.5, ru-ep3 (2,372 records), ru-ep45 (2,474), fr-ep54 (2,992) and phoenix-ep63 (4,511).

## Other episodes

The layout is the original client generation's. `Monsters::parse` fails on kr-ep8, fawkes-v68 and cn-ep4 in the version matrix. When it does not read, the editor says so in one note, and the monster names, levels and rules are off. The table view reads kr-ep8 (4,776 records) and fawkes-v68 (4,774) with `monster-36` (a name and a 36-byte block), FIT-ONLY, see below. cn-ep4 has no fit: neither `monster-31` nor `monster-36` fits it, so the file is shown as hex.

## FIT-ONLY layouts

`monster-36` is a FIT-ONLY layout: the record is a text and a fixed block of 36 bytes, and it reads the kr-ep8 and the fawkes-v68 file to the last byte, but it was not read from any client's own loader (their `game.exe` is packed). The table view and `shaiya sdata table` say "FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly."; the JSON has `"status": "fit-only"`; the columns are `id`, `s0` and `f<offset>`. Multi-byte widths were taken over from `monster-31` only where the values aligned with the verified client's, and the five bytes that `monster-31` does not have are not interpreted.

**Upgrade path.** The layout becomes loader-verified when it is read from those clients' own loaders (an unpacked `game.exe` build or a memory dump) and still reads every Monster file of that client to the last byte; its clients then move from `fit_only` to `loader` in the layout file, confirmed names replace the positions, and the matrix cell becomes OK.
