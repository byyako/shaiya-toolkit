# `.3DO` meshes

`.3DO` files are meshes that carry no bones: a list of vertices, the triangles between them and, in one file only, a set of vertex frames. One of them is `Item/3DO/arrow.3DO`.

All numbers are little-endian and there is no header. A string is `i32 length` and that many bytes. A file is read only when it ends exactly after the last frame.

## Layout as read

```
i32 length, then that many texture-name bytes   kept raw: the NUL and anything after it are kept; usually empty
i32 vertex count; vertices                      32 bytes each
i32 face count;   faces                         3 x u16 vertex indices each
i32 frame_rate                                  inferred: frames per second
i32 frame_count
per frame:
  i32 key
  24 bytes per vertex of the mesh               f32 position[3], f32 normal[3]: the whole vertex set of that frame
```

A vertex is 32 bytes, in this order: position (3 x f32), normal (3 x f32) and texture coordinates (2 x f32).

The toolkit keeps floats exactly as stored (NaN bits included) and the texture name as raw bytes (displayed as text up to the first NUL), so a file written back is the same bytes as the file read. Before it allocates, the reader checks that the counts fit in the bytes left (for the frames: `frame count x (4 + 24 x vertex count)`). Writing is in memory only (`Mesh3do::to_bytes`): no program writes a `.3DO` file.

`frame_rate` is inferred to be the number of frames per second; it was not checked in the game.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `mesh3do` | parses, writes back in memory, names the texture, gives the vertices of a frame (the positions and normals of that frame with the texture coordinates of the mesh) |

In Shaiya Toolkit's Archive tab a `.3DO` shows its mesh in plain grey with the wire, under "3DO mesh: N vertices, N triangles" (a texture the file names is not looked up, and the note says so). A file with frames (US EP4.5: only `Item/3DO/arrow.3DO`) plays them at its frame rate, shown as "N frames at N per second (inferred from the file; not checked in the game)", with Play/Pause, a slider and Loop; a rate that is not positive plays at 30 frames per second and says so. A file that does not read to the last byte shows its error and the bytes as hex, also when the bytes look like text. `shaiya anim info` prints one line per file (vertices, triangles, frames, rate as inferred); `--json` has the same as keys. `shaiya anim validate` and `compare` read `.ANI` files only. Validated offline on US EP4.5 files.

## JSON (export and import)

The toolkit's library can turn a `.3DO` into a JSON text (`shaiya-toolkit-3do-v1`) and that text back into the file. An unedited file comes back byte for byte (US EP4.5: all 271 `.3DO` files; validated offline; the eight other matrix clients have their index only and were not read). The JSON goes in through the same Replace and `archive replace` paths as any file: nothing is written into a client folder.

| Key | Content |
|---|---|
| `format` | `shaiya-toolkit-3do-v1`; any other name is refused, and a newer version (`-v2`) is refused as "made by a newer toolkit" |
| `source` | label only (ignored on import) |
| `inferred` | label: `frame_rate` is frames per second, which is inferred (ignored on import) |
| `columns` | `vertices`: `x, y, z, nx, ny, nz, u, v`; `frame_vertices`: `x, y, z, nx, ny, nz`; may be left out, but when present they must equal these lists |
| `texture_hex` | the texture name as stored, as hex |
| `vertices` | `[x, y, z, nx, ny, nz, u, v]` per vertex |
| `faces` | `[a, b, c]` per triangle |
| `frame_rate` | integer (inferred) |
| `frames` | `{"key": n, "vertices": [[x, y, z, nx, ny, nz], ...]}` per frame; each frame must hold one record per vertex |

- Each vertex, frame record and triangle is written on one line. Floats are written as for `.3DC`: the shortest number, or `"bits:XXXXXXXX"`.
- A frame with a missing or extra vertex, counts over the reader's limits, unknown keys, other columns and a newer format are refused. Hex is read in either letter case.
- The bytes made from a JSON are read back by the `.3DO` reader before they are returned.

## Verified on

US EP4.5 (patch 108), read only: all 271 `.3DO` files read to the last byte and write back byte for byte. Three of them name a texture; the others have an empty texture name. Only one file has frames: `Item/3DO/arrow.3DO`, with `frame_rate` 20 and 11 frames; the other 270 have 0 frames. The opt-in test `every_3do_of_the_us_client_reads_and_writes_back` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.3DO` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
