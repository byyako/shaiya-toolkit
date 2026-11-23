# Vertex-animated models (`.VANI`)

`.VANI` files are models whose animation is a complete set of vertices for every step, rather than bones moving a skin. Every item of the model has the same number of steps, one list of faces shared by all of them, and its own vertices for each step. On US EP4.5 they are all in the `Entity/VAni` folder of the archive.

All numbers are little-endian and there is no header. A string is `i32 length` and that many bytes. A file is read only when it ends exactly after the last field.

## Layout as read

```
f32 sphere centre[3]
f32 sphere radius
f32 box min[3], max[3]
i32 item count
i32 step count
i32 frame_ms                     inferred: milliseconds per step
per item:
  string texture                 the texture name; may be empty; kept as raw bytes
  i32 face count; faces          3 x u16 each, shared by every step
  i32 vertices per step
  vertices                       36 bytes x vertices per step x step count, step by step
f32 box2 min[3], max[3]
i32 collision flag               0, or 1 followed by a collision mesh
  collision mesh:                i32 point count, f32[3] x point count, i32 face count, faces (the same as in a .smod)
```

A vertex is 36 bytes: position (3 x f32), normal (3 x f32), a diffuse colour (u32) and texture coordinates (2 x f32).

The toolkit keeps floats exactly as stored (NaN bits included) and texture names as raw bytes (displayed as text up to the first NUL), so a file written back is the same bytes as the file read. Before it allocates, the reader checks that `vertices per step x step count x 36` fits in the bytes left. Writing is in memory only (`Vani::to_bytes`): no program writes a `.VANI` file.

`frame_ms` is inferred to be the time of one step in milliseconds; it was not checked in the game.

## Collision mesh

On US EP4.5 the collision flag is 0 in all 66 files, so no real file exercises the collision branch. The toolkit reads it with the layout of the `.smod` collision mesh, from a synthetic file only (a test), and only for the flag values 0 and 1: any other value is refused, not guessed.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `vani` | parses, writes back in memory, gives the vertices of an item at a step and the texture name |
| Shaiya Toolkit, Archive tab | previews it: the model picture of every step (textures looked up under `Entity/Texture` as for a `.smod`, a missing one named and drawn grey), played one step per `frame_ms` milliseconds with Play/Pause, a slider and Loop; the header says "about N ms per step (inferred from the file; not checked in the game)". At most 241 pictures are prepared on a worker thread, one every few steps for a longer file. A file that does not read to the last byte shows its error and the bytes as hex, also when the bytes look like text |
| `shaiya anim info` | one line per file (items, vertices per step, triangles, steps, `frame_ms` as inferred); `--json` has the same as keys. `shaiya anim validate` and `compare` read `.ANI` files only |

The preview and the player are validated offline on US EP4.5 files; the eight other matrix clients have their index only.

## JSON (export and import)

The toolkit's library can turn a `.VANI` into a JSON text (`shaiya-toolkit-vani-v1`) and that text back into the file. An unedited file comes back byte for byte (US EP4.5: all 66 `.VANI` files; validated offline; the eight other matrix clients have their index only and were not read). The JSON goes in through the same Replace and `archive replace` paths as any file: nothing is written into a client folder.

| Key | Content |
|---|---|
| `format` | `shaiya-toolkit-vani-v1`; any other name is refused, and a newer version (`-v2`) is refused as "made by a newer toolkit" |
| `source` | label only (ignored on import) |
| `inferred` | label: `frame_ms` is milliseconds per step, which is inferred (ignored on import) |
| `columns` | `vertices`: `x, y, z, nx, ny, nz, diffuse, u, v`; may be left out, but when present it must equal this list |
| `sphere_centre`, `sphere_radius`, `box_min`, `box_max` | the sphere and the box |
| `step_count`, `frame_ms` | integers (`frame_ms` is inferred) |
| `items` | one object per item |
| `items[i].texture_hex` | the texture name as stored, as hex |
| `items[i].faces` | `[a, b, c]` per triangle, shared by every step |
| `items[i].verts_per_step` | integer |
| `items[i].vertices` | `[x, y, z, nx, ny, nz, diffuse, u, v]` per vertex, step by step; the number of records must be `verts_per_step` times `step_count`. `diffuse` is the stored colour as an integer (4294967295 is `0xFFFFFFFF`) |
| `box2_min`, `box2_max` | the second box |
| `collision` | `null` (every file of US EP4.5), or `{"points": [[x, y, z], ...], "faces": [[a, b, c], ...]}`; the key must be present |

- Each vertex, point and triangle is written on one line. Floats are written as for `.3DC`: the shortest number, or `"bits:XXXXXXXX"` for NaN, infinity or a value that has no such number.
- A vertex list that is not `verts_per_step` times `step_count`, counts over the reader's limits, unknown keys, other columns and a newer format are refused. Hex is read in either letter case.
- The bytes made from a JSON are read back by the `.VANI` reader before they are returned.

## Verified on

US EP4.5 (patch 108), read only: all 66 `.VANI` files read to the last byte and write back byte for byte. They have 1 to 8 items (7 different counts: 1, 2, 3, 4, 5, 7 and 8) and 11 to 101 steps. `frame_ms` is 33 in 8 files, 66 in 47 and 100 in 11. In 16 files a vertex has a diffuse colour other than `0xFFFFFFFF`. In all 66 the sphere centre equals the centre of the box and the sphere radius equals half the box diagonal (within 0.1 % or 0.001). The collision flag is 0 in every file. The opt-in test `every_vani_of_the_us_client_reads_and_writes_back` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.VANI` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
