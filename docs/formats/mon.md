# `.MON` monster part lists

A `.MON` file lists, per monster, NPC, vehicle or similar model, the mesh and texture names of its parts. With the [`.MLT`](mlt.md) and [`.ITM`](itm.md) lists it is where a `.3DC` mesh's texture name is kept (the mesh names none).

All numbers are little-endian. A string is `i32 length` and that many bytes; in this file the strings carry no NUL.

## Layout as read

```
"MO2"                                   signature
i32 nRec; nRec records
  str name
  u8 flag
  str slot[17]                          17 name slots; most are empty
  i32 nParts; nParts x { str mesh, str texture }
  f32 size
  i32 nAttach; nAttach x { i32 a, i32 b }
```

A file is read only when it ends exactly after the last record. The toolkit keeps every name as the raw bytes stored and every number as read (the bits of `size` included), so a file written back is the same bytes as the file read; a name is decoded from code page 949 for display. Before it allocates, the reader checks that every count is at most 65,536 and fits in the bytes left (a record is at least 85 bytes), and that a name is at most 4,096 bytes. Another episode's signature (`MO4`, ...) is refused with "a layout of another episode, not read", never half-read.

| Field | Evidence |
|---|---|
| Layout (the records, 17 slots, the part and attachment lists) | validated on file contents (US EP4.5): `every_list_file_of_the_us_client_reads_and_writes_back` |
| `flag`, the meaning of the 17 slots | unknown: kept as read |
| `size` | inferred: a scale of the model |
| `attachments` `(a, b)` | inferred: a bone index and an effect scene of the monster effect file |

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `modellist` | `Mon::parse` reads it, `Mon::to_bytes` builds the bytes in memory (the tests), `name_text` decodes a name |
| `shaiya info` | names the file with its counts ("MON list, N records, N parts") |

No program writes a `.MON` file. `cargo xtask guard` refuses a tracked file with this extension.

## Verified on

US EP4.5 (patch 108), read only: all 16 `.MON` files read to the last byte and write back byte for byte, 785 records and 2,005 parts in all. The opt-in test `every_list_file_of_the_us_client_reads_and_writes_back` measures these numbers (validated offline).

## Other episodes

The eight other matrix clients have their index only: their `.MON` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
