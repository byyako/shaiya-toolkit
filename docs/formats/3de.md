# Effect meshes (`.3DE`)

`.3DE` files are meshes that effects use: vertices, triangles and, in most files, a block of frames. On US EP4.5, 468 of the 469 are in the `Effect/3DE` folder of the archive and 1 is in `Effect/DDS`. The effect files themselves (`.EFT`) are not read by any program yet; this page covers only the mesh.

All numbers are little-endian and there is no header. A string is `i32 length` and that many bytes. A file is read only when it ends exactly after its last field.

## Layouts as read

Two layouts are read. The toolkit tries each and accepts a file only when exactly one of them reads it to the last byte; a file that both read would be refused as "ambiguous layout" (the two cannot both read a file, because the standard layout needs 8 more bytes than the static one).

| Layout | Content |
|---|---|
| standard | the mesh and a frame block (the block may hold 0 frames) |
| static | the same, ending right after the faces: no frame block |

```
i32 length, then that many texture-name bytes   kept raw: the NUL and anything after it are kept; may be empty
i32 vertex count; vertices                      24 bytes each
i32 face count;   faces                         3 x u16 vertex indices each
                                                (the static layout ends here)
i32 max_key                                     inferred
i32 frame_count
per frame:
  i32 key
  20 bytes per vertex of the mesh               f32[3] (inferred: a position change), f32 u, f32 v
```

A vertex is 24 bytes: position (3 x f32), an `i32` (-1 in every file read; kept as read) and texture coordinates (2 x f32).

The toolkit keeps floats exactly as stored (NaN bits included) and the texture name as raw bytes (displayed as text up to the first NUL), so a file written back is the same bytes as the file read. Before it allocates, the reader checks that the counts fit in the bytes left. Writing is in memory only (`Mesh3de::to_bytes`): no program writes a `.3DE` file.

The meanings of `max_key` and of the 20-byte frame entries are inferred and were not checked in the game.

## Files that are refused

A file that neither layout reads to the last byte is never half-read: the reader refuses it with "does not follow the .3DE layout the toolkit reads" and the reason the standard layout gave, and an empty file is reported as empty. On US EP4.5 two files are refused this way: `004.3de` and `Deathknight007.3DE`, both in `Effect/3DE`. The toolkit does not guess their layout.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `mesh3de` | parses the two layouts, writes back in memory, names the texture, counts the frames |

In Shaiya Toolkit's Archive tab a `.3DE` shows its mesh in plain grey with the wire, under "Effect mesh: N vertices, N triangles", and says how many frames the file holds: "N frames (not played: their meaning is inferred)", "0 frames", or "no frame block: the file ends after the faces" for the static layout. The frames are not played. A `.3DE` that follows neither layout is shown as a failed preview with the reason ("does not follow the .3DE layout the toolkit reads" and why the standard layout failed) and its bytes as hex, also when the bytes look like text; an empty file shows as empty. `shaiya anim info` prints one line per file (vertices, triangles, layout, frames); `--json` adds `max_key` (inferred) and the texture name. `shaiya anim validate` and `compare` read `.ANI` files only. Validated offline on US EP4.5 files.

## Verified on

US EP4.5 (patch 108), read only: the index lists 469 `.3DE` files. 467 read to the last byte and write back byte for byte: 466 in the standard layout (349 of them with at least one frame) and 1 in the static layout (`Effect/3DE/d_dragonstone03.3de`). The other 2 are the refused files named above. The `i32` of every vertex is -1 in all 467. The opt-in test `every_3de_of_the_us_client_reads_or_is_named` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.3DE` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
