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

The toolkit keeps floats exactly as stored (NaN bits included) and the texture name as raw bytes (displayed as text up to the first NUL), so a file written back is the same bytes as the file read. Before it allocates, the reader checks that the counts fit in the bytes left. `Mesh3de::to_bytes` writes in memory; the only program that writes a `.3DE` file is `shaiya anim import`, from its JSON and only to the `-o` file.

The meanings of `max_key` and of the 20-byte frame entries are inferred and were not checked in the game.

## Files that are refused

A file that neither layout reads to the last byte is never half-read: the reader refuses it with "does not follow the .3DE layout the toolkit reads" and the reason the standard layout gave, and an empty file is reported as empty. On US EP4.5 two files are refused this way: `004.3de` and `Deathknight007.3DE`, both in `Effect/3DE`. The toolkit does not guess their layout.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `mesh3de` | parses the two layouts, writes back in memory, names the texture, counts the frames |
| `shaiya anim export` and `import` (command line) | `export` turns a file into JSON and `import` turns the JSON back into the file; both write only their `-o` file ([JSON](#json-export-and-import)) |
| Shaiya Toolkit, Archive tab | Export as JSON... writes the JSON of the selected `.3DE` file to a file outside every client folder ([JSON](#json-export-and-import)); the Toolkit imports nothing |

In Shaiya Toolkit's Archive tab a `.3DE` shows its mesh in plain grey with the wire, under "Effect mesh: N vertices, N triangles", and says how many frames the file holds: "N frames (not played: their meaning is inferred)", "0 frames", or "no frame block: the file ends after the faces" for the static layout. The frames are not played. A `.3DE` that follows neither layout is shown as a failed preview with the reason ("does not follow the .3DE layout the toolkit reads" and why the standard layout failed) and its bytes as hex, also when the bytes look like text; an empty file shows as empty. `shaiya anim info` prints one line per file (vertices, triangles, layout, frames); `--json` adds `max_key` (inferred) and the texture name. `shaiya anim validate` and `compare` read `.ANI` files only. Validated offline on US EP4.5 files.

## JSON (export and import)

The toolkit's library can turn a `.3DE` into a JSON text (`shaiya-toolkit-3de-v1`) and that text back into the file. An unedited file comes back byte for byte (US EP4.5: 467 of the 469 `.3DE` files; validated offline; the eight other matrix clients have their index only and were not read). The 2 files the toolkit does not read (`004.3de` and `Deathknight007.3DE`) are not exported: they are refused with the reader's reason. The library writes no file into a client folder: the file made from the JSON is what goes in, through Replace or `archive replace` like any other file.

| Key | Content |
|---|---|
| `format` | `shaiya-toolkit-3de-v1`; any other name is refused, and a newer version (`-v2`) is refused as "made by a newer toolkit" |
| `source` | label only (ignored on import) |
| `inferred` | labels: `max_key` and the frame entries (`dx, dy, dz` as a position change) are inferred (ignored on import) |
| `columns` | `vertices`: `x, y, z, field, u, v`; `frame_vertices`: `dx, dy, dz, u, v`; may be left out, but when present they must equal these lists |
| `layout` | `standard` or `static`; must agree with `frame_block` |
| `texture_hex` | the texture name as stored, as hex |
| `vertices` | `[x, y, z, field, u, v]` per vertex; `field` is the stored integer (-1 in every file read), kept |
| `faces` | `[a, b, c]` per triangle |
| `frame_block` | `null` for `static`, otherwise `{"max_key": n, "frames": [{"key": n, "vertices": [[dx, dy, dz, u, v], ...]}]}`; the key must be present, and each frame holds one record per vertex |

- Each vertex, frame record and triangle is written on one line. Floats are written as for `.3DC`: the shortest number, or `"bits:XXXXXXXX"`.
- A layout that does not agree with its frame block, a frame with a missing or extra entry, counts over the reader's limits, unknown keys, other columns and a newer format are refused. An empty `.3DE` is not exported.
- The bytes made from a JSON are read back by the `.3DE` reader before they are returned, and must read with the same layout.

## Verified on

US EP4.5 (patch 108), read only: the index lists 469 `.3DE` files. 467 read to the last byte and write back byte for byte: 466 in the standard layout (349 of them with at least one frame) and 1 in the static layout (`Effect/3DE/d_dragonstone03.3de`). The other 2 are the refused files named above. The `i32` of every vertex is -1 in all 467. The opt-in test `every_3de_of_the_us_client_reads_or_is_named` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.3DE` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
