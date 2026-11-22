# Skinned meshes (`.3DC`)

`.3DC` files are skinned meshes: vertices that each follow one or two bones of a skeleton, plus the triangles between them. They live in the `3DC` folders of the archive (for example `Character/<race>/3DC`, `Monster/3DC`). The bones are the ones of an animation (`.ANI`, see [ani.md](ani.md)); the format has no bone names, so a mesh and an animation are matched by bone index.

All numbers are little-endian and there is no header. A file is read only when it ends exactly after the last face.

## Layouts as read

Three layouts are known. The toolkit tries each and accepts a file only when exactly one of them reads it to the last byte; a file that two layouts read is refused as "ambiguous layout".

| Layout | Content |
|---|---|
| skinned | one mesh with a bone block |
| skinned + second mesh | two skinned meshes back to back |
| cloak (no bone block) | one mesh without the bone block |

```
i32 length, then that many texture-name bytes   kept raw: the NUL and anything after it are kept
i32 bone count; f32 matrix[16] x bone count     the bone block (absent in a cloak); row-major
i32 vertex count; vertices                      40 bytes each
i32 face count;   faces                         3 x u16 vertex indices each
```

A vertex is 40 bytes, in this order:

| Field | Type | Meaning |
|---|---|---|
| position | f32 x 3 | x, y, z |
| weight | f32 | the share of `bone1`; the rest goes to `bone2` |
| `bone1`, `bone2` | u8, u8 | bone indices |
| `pad` | u16 | not always 0: kept as read |
| normal | f32 x 3 | |
| uv | f32 x 2 | texture coordinates |

The matrices of the bone block are the mesh's own inverse bind pose. They are not the inverse of an animation's reference matrices.

The toolkit keeps floats exactly as stored (NaN bits included) and the texture name as raw bytes (displayed as text up to the first NUL), so a file written back is the same bytes as the file read. Writing is in memory only (`C3dc::to_bytes`): no program writes a `.3DC` file.

Cloak meshes: the layout reads all 48 of them to the last byte; how the game moves them was not checked.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `mesh3dc` | parses the three layouts, writes back in memory, names the texture and the highest bone a vertex uses |

No program shows a `.3DC` yet: the Archive tab lists, extracts and replaces it like any file and shows it as hex.

## Verified on

US EP4.5 (patch 108), read only: all 2,431 `.3DC` files read to the last byte and write back byte for byte, 2,382 with the skinned layout, 1 with a second mesh and 48 as cloaks, none ambiguous; every vertex bone index of a skinned mesh is below the bone count of its own mesh; the `pad` field is non-zero in some vertices; the matrices of the torso part of each of the 16 character rigs are not the inverse of the reference matrices of that rig's first `.ANI` (the opt-in tests print the counts).

## Other episodes

The eight other matrix clients have their index only: their `.3DC` files are counted (2,317 to 6,534), none was read. Later episodes may use another vertex layout; not checked.
