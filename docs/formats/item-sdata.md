# `Item.SData`

`Item.SData` is the client's item table (`Item/Item.SData` in the archive). The file is [SEED-wrapped](sdata-wrapper.md) in most clients; this page is about the plaintext. Shaiya Toolkit's SData tab and [`shaiya sdata table`](../tools/shaiya.md#shaiya-sdata) show it as a table. Both are read only.

## Layout as read

All numbers are little-endian. Strings are a `u32` byte length followed by that many bytes (code page 949 on the original client).

| Part | Type |
|---|---|
| bank count | `u32` (the client keeps only the low byte) |
| per bank: record count | `u32` (0 is an empty bank) |
| per record: name | string |
| per record: description | string |
| per record: fixed block | 81 bytes (variant `item-81`) or 132 bytes (variant `item-132`) |

The file must end exactly after the last record, and the bank count must be at most 255. A layout is used for a file only when it reads the whole file this way; a file that does not read is shown as hex.

### The 81-byte block (`item-81`)

Fields at their offsets in the block. A name is given only where the meaning is known; every other field is shown as `f<offset>` (for example `f2`, `f22`).

| Offset | Type | Name |
|---|---|---|
| 0 | `u8` | `type` |
| 1 | `u8` | `type_id` |
| 4 | `u16` | `req_level` |
| 24, 26, 28 | `u16` | `req_wis`, `req_luc`, `req_vg` |
| 32 | `u8` | `range` |
| 35 | `u8` | `special` |
| 36 | `u8` | `slot` |
| 37 | `u16` | `quality` |
| 47, 49, 51 | `u16` | `hp`, `sp`, `mp` |
| 67, 71 | `u32` | `buy`, `sell` |
| 80 | `u8` | `stack` |

The table view adds a `bank` column (the bank number, from 0) in front of `name`, `description` and the fields.

### The 132-byte block (`item-132`)

The block is 51 bytes longer in the 81-byte form's terms: `range` is a `u16` (so every field after it is one byte further on: `special` at 36, `slot` at 37, `quality` at 38, `hp`, `sp`, `mp` at 48, 50, 52, `buy` and `sell` at 68 and 72, `stack` at 81), and 50 more bytes follow the 81-byte block's last field, shown as `f82` to `f128`.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-sdata` | reads the table with the layout that fits the file (`item-81`, `item-132` or the FIT-ONLY `item-134`) |
| `shaiya sdata table` and the SData tab's View table | rows and columns, filter, CSV, JSON |

The file is only read.

## Verified on

`item-81` was derived from the client's own table loader and checked against the whole file on US EP4.5 (patch 108, 16,045 records in the client's file), ru-ep3 (15,612 records), ru-ep45 (16,559) and fr-ep54 (17,090, 101 banks). `item-132` was derived and checked the same way on phoenix-ep63 (22,697 records, 150 banks). The numbers are the measured rows of the [version matrix](../version-matrix.md).

## Other episodes

The layout of a client is not assumed. `auto` tries every variant and uses the one that reads the file to its last byte; when none does, the file is shown as hex. `item-81` also reads the file of cn-ep4 (16,047 records) and `item-134` the files of kr-ep8 (25,245 records) and fawkes-v68 (24,990): fits of the data, not checked against those clients' loaders (FIT-ONLY, below).

## FIT-ONLY layouts

A layout file says, in its `evidence` object, which clients it was read from the loader of (`loader`) and which clients' files it only reads to the last byte (`fit_only`). A layout with no `loader` client is **FIT-ONLY**, and a client listed under `fit_only` of a loader-verified layout is FIT-ONLY on that layout. FIT-ONLY is never the same as verified: no loader of that client was read (its `game.exe` is packed). The viewers say so: the table view and `shaiya sdata table` (on stderr) show "FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly.", the JSON has `"status": "fit-only"`, and the [version matrix](../version-matrix.md) cell says `FIT-ONLY` with the record count.

For Item: `item-134` is the same container as `item-81` (a bank count, then per bank a record count and that many records of two texts and a fixed block) with a 134-byte block; 134 is the only block size that reads the kr-ep8 and the fawkes-v68 file to the last byte. Its columns are `bank`, `s0`, `s1` and `f<offset>` for the block; no field has a meaning. Multi-byte widths were taken over from `item-81` only where the values aligned with a verified client's, so a value may still be split wrongly. `item-81` on cn-ep4 is a layout verified on other clients that reads that client's file: FIT-ONLY there.

**Upgrade path.** A FIT-ONLY layout becomes loader-verified when it is read from that client's own loader (an unpacked `game.exe` build or a memory dump of that client) and still reads every file of that table to the last byte. Then the client moves from `fit_only` to `loader` in the layout file, the confirmed field names replace the positions, and the matrix cell becomes OK. Until then the layout stays FIT-ONLY.
