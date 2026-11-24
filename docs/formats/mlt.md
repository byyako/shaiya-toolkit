# `.MLT` character part lists

An `.MLT` file lists the meshes and textures of the character models: a list of `.3DC` names, a list of `.DDS` names and a list of records that pair one of each with an alpha flag. A `.3DC` mesh names no texture itself (the name is empty in 2,429 of the 2,431 `.3DC` files of `us-108`), so this list, the [`.MON`](mon.md) list and the [`.ITM`](itm.md) list are where a model's texture name is kept.

All numbers are little-endian. A string is `i32 length` and that many bytes.

## Layout as read

```
"ML2"                                   signature
i32 n3DC; n3DC strings                  the .3DC names; each length counts the NUL that ends the name
i32 nDDS; nDDS strings                  the .DDS names; the same
i32 nRec; nRec records                  12 bytes each
  i32 mesh                              index into the .3DC names
  i32 texture                           index into the .DDS names
  i32 alpha_flag                        0 or 1 in every file read
```

A file is read only when it ends exactly after the last record. The toolkit keeps every name as the raw bytes stored (the NUL included) and every number as read, so a file written back is the same bytes as the file read; a name is decoded from code page 949 for display, up to its first NUL. An index outside its list is kept as read and has no name. Before it allocates, the reader checks that every count is at most 65,536 and fits in the bytes left, and that a name is at most 4,096 bytes. Another episode's signature (`ML3`, ...) is refused with "a layout of another episode, not read", never half-read.

| Field | Evidence |
|---|---|
| Layout (counts, strings with a NUL, the three words of a record) | validated on file contents (US EP4.5): `every_list_file_of_the_us_client_reads_and_writes_back` |
| `mesh`, `texture` are indices into the two lists | validated on file contents (US EP4.5): no record points outside its lists, same test |
| `alpha_flag` | values validated on file contents (US EP4.5): 0 in 638 records and 1 in 1,538; what the flag means is inferred only and nothing was read from the game for it |

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `modellist` | `Mlt::parse` reads it, `Mlt::to_bytes` builds the bytes in memory (the tests), `name_text` decodes a name |
| `shaiya info` | names the file with its counts ("MLT list, N records, N mesh names, N texture names") |

No program writes an `.MLT` file. `cargo xtask guard` refuses a tracked file with this extension.

## Verified on

US EP4.5 (patch 108), read only: all 112 `.MLT` files read to the last byte and write back byte for byte, 2,176 records in all. The opt-in test `every_list_file_of_the_us_client_reads_and_writes_back` measures these numbers (validated offline).

## Other episodes

The eight other matrix clients have their index only: their `.MLT` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
