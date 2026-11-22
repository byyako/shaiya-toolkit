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

## How the toolkit poses a mesh

The pose is the toolkit's own statement of the math, an approximation of the game's playback, validated offline (nothing here was checked in the game). Matrices are 4 x 4, stored row by row, used with row vectors (`v' = v * M`), with the translation in the last row; `A * B` means "A, then B".

- A rotation key `(x, y, z, w)` becomes a rotation matrix in that row-vector form (a quarter turn about y takes `(1, 0, 0)` to `(0, 0, -1)`).
- `Local` of a root bone is its reference matrix; `Local` of any other bone is its reference matrix times the inverse of its parent's (a general 4 x 4 inverse; a singular or non-finite matrix gives NaN and the bone is counted).
- At time `t` (in key units) a bone's `Local` gets the sampled rotation in its upper 3 x 3 and the sampled translation in its last row, when it has such keys. Before a track's first key its first value holds and at or after its last key its last value holds; in between, rotations are slerped the short way and translations are linear. A bone's world matrix is its `Local` times its parent's world matrix.
- A `.3DC` vertex with weight `w` and bones `b1`, `b2` moves with `w * (B[b1] * World[b1]) + (1 - w) * (B[b2] * World[b2])`, where `B` are the mesh's own matrices. A vertex that names a bone the animation (or the mesh) does not have keeps its rest position and is counted.

Only plain interpolation is modelled; the game may differ.

On US EP4.5, each of the 16 character rigs posed with its first `<rig>_000_` animation and the default parts of that rig (torso, lower, hand, boots, face, hair) at key 0 and at the end key keeps finite positions, leaves no vertex unposed and keeps a bounding box between 0.90 and 0.98 times that of the rest mesh (opt-in test).

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `mesh3dc` | parses the three layouts, writes back in memory, names the texture and the highest bone a vertex uses |
| `shaiya-clientdata` `render` | `skin` poses a mesh, `c3dc_picture` draws it at rest and `anim_frames` pre-renders an animation on it |
| Shaiya Toolkit, Archive tab | previews a `.3DC` at rest, and uses it as the mesh of an `.ANI` preview (see [ani.md](ani.md#in-the-archive-tab)) |

No program writes a `.3DC` file. Selecting a `.3DC` in the Archive tab shows its first mesh at rest, in plain grey with the wire, under a header: "Skinned mesh: N vertices, N triangles, N bones", "Cloak mesh without bones: N vertices, N triangles", or, for the layout with a second mesh, the same with "a second mesh follows (N vertices); the picture shows the first". A weak line says "shown in the mesh's own rest pose, plain grey (the mesh file names no texture)", or "(textures are not looked up for skinned meshes)" for a mesh that does name one. A `.3DC` is never shown as text, even when its bytes look like text: a file that does not read to the last byte (or that two layouts read) shows the reader's error and the bytes as hex.

## Verified on

US EP4.5 (patch 108), read only: all 2,431 `.3DC` files read to the last byte and write back byte for byte, 2,382 with the skinned layout, 1 with a second mesh and 48 as cloaks, none ambiguous; every vertex bone index of a skinned mesh is below the bone count of its own mesh; the `pad` field is non-zero in some vertices; the matrices of the torso part of each of the 16 character rigs are not the inverse of the reference matrices of that rig's first `.ANI` (the opt-in tests print the counts).

## Other episodes

The eight other matrix clients have their index only: their `.3DC` files are counted (2,317 to 6,534), none was read. Later episodes may use another vertex layout; not checked.
