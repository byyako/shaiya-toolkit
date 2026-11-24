# `.ITM` item model lists

An `.ITM` file lists the `.3DO` meshes and `.DDS` textures of the item models and holds a record per item model that points into the two lists. With the [`.MLT`](mlt.md) and [`.MON`](mon.md) lists it is where a model's texture name is kept.

All numbers are little-endian. A string is `i32 length` and that many bytes; in this file the strings carry no NUL.

## Layout as read

```
"ITM"                                   signature
i32 n3DO; n3DO strings                  the .3DO names
i32 nDDS; nDDS strings                  the .DDS names
i32 nRec; nRec records
  i32 model                             index into the .3DO names
  i32 texture                           index into the .DDS names
  i32 glow
  i32 unk1
  i32 format                            0 or 1
  i32 unk2
  4 x i32                               only when format is 1
```

A file is read only when it ends exactly after the last record. The toolkit keeps every name as the raw bytes stored and every number as read, so a file written back is the same bytes as the file read; a name is decoded from code page 949 for display. Before it allocates, the reader checks that every count is at most 65,536 and fits in the bytes left, and that a name is at most 4,096 bytes. A record whose `format` is not 1 has no extra words. Another episode's signature (`IT2`, ...) is refused with "a layout of another episode, not read", never half-read.

| Field | Evidence |
|---|---|
| Layout (the two name lists, six words, four more when `format` is 1) | validated on file contents (US EP4.5): `every_list_file_of_the_us_client_reads_and_writes_back` |
| `model`, `texture` are indices into the two lists | validated on file contents (US EP4.5): no record points outside its lists, same test |
| `format` | validated on file contents (US EP4.5): 0 in 462 records, 1 in 1,078; the four extra words follow exactly when it is 1 |
| `glow` | values validated on file contents (US EP4.5): -1 in 515 records, 0 in 431, 1 in 394, 2 in 100 and 3 in 100; what it means is inferred only and nothing was read from the game for it |
| `unk1`, `unk2`, the four extra words | unknown: kept as read |

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `modellist` | `Itm::parse` reads it, `Itm::to_bytes` builds the bytes in memory (the tests), `name_text` decodes a name |
| `shaiya info` | names the file with its counts ("ITM list, N records, N mesh names, N texture names") |

No program writes an `.ITM` file. `cargo xtask guard` refuses a tracked file with this extension.

## Verified on

US EP4.5 (patch 108), read only: all 17 `.ITM` files read to the last byte and write back byte for byte, 1,540 records in all. The opt-in test `every_list_file_of_the_us_client_reads_and_writes_back` measures these numbers (validated offline).

## Other episodes

The eight other matrix clients have their index only: their `.ITM` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
