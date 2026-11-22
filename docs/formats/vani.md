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

The Archive tab and `shaiya anim` do not read `.VANI` yet: the Archive tab shows the bytes as hex, like any format that has no preview.

## Verified on

US EP4.5 (patch 108), read only: all 66 `.VANI` files read to the last byte and write back byte for byte. They have 1 to 8 items (7 different counts: 1, 2, 3, 4, 5, 7 and 8) and 11 to 101 steps. `frame_ms` is 33 in 8 files, 66 in 47 and 100 in 11. In 16 files a vertex has a diffuse colour other than `0xFFFFFFFF`. In all 66 the sphere centre equals the centre of the box and the sphere radius equals half the box diagonal (within 0.1 %). The collision flag is 0 in every file. The opt-in test `every_vani_of_the_us_client_reads_and_writes_back` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.VANI` files are counted (see the [version matrix](../version-matrix.md)), none was read. Whether they read is not checked.
