# Item meshes (`.3DO`)

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

No program shows a `.3DO` yet: the Archive tab has no preview for it (it shows the bytes as hex and says that there is no preview yet), and `shaiya anim` does not read it.

## Verified on

US EP4.5 (patch 108), read only: all 271 `.3DO` files read to the last byte and write back byte for byte. Three of them name a texture; the others have an empty texture name. Only one file has frames: `Item/3DO/arrow.3DO`, with `frame_rate` 20 and 11 frames; the other 270 have 0 frames. The opt-in test `every_3do_of_the_us_client_reads_and_writes_back` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.3DO` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
