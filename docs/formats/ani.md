# Animations (`.ANI`)

`.ANI` files are skeletal animations: for each bone of a skeleton, a list of rotation keys and a list of translation keys over time. They live in the `ANI` folders of the archive (for example `Character/<race>/ANI`, `Monster/ANI`). A skinned model (`.3DC`) is moved by the bones of the animation it is played with; the format has no bone names, so the two are matched by bone index.

All numbers are little-endian and there is no header.

## Layout as read

```
i32 first field                  -1 in 7 files and 0 in all others on US EP4.5 (inferred: the start key)
i32 end key
u16 bone count
per bone:
  i32 parent                     -1 for a root, else the index of an earlier bone
  f32 reference matrix[16]       64 bytes, row-major
  i32 nRot,   nRot   x { i32 key; f32 x, y, z, w }    rotation keys (a quaternion)
  i32 nTrans, nTrans x { i32 key; f32 x, y, z }       translation keys
```

Key times are in units of 1/30 second (30 per second is the game's default rate). The file must end at the last byte. There are no bone names: meshes are matched to bones by index.

The toolkit keeps floats exactly as stored, NaN bits included, so a file written back is the same bytes as the file read. Reference matrices that hold NaN are kept bit for bit.

## Rules a replacement must keep

A replacement must keep the bone count, the order and parents of the bones and all 64 bytes of every reference matrix of the file it replaces. Almost every file of a rig has its own reference matrices, so copy them from the exact file you replace.

The toolkit also asks for the same first field as the file it replaces (inferred to be the start key; not checked in the game).

## What the toolkit checks (validation codes)

`shaiya-clientdata` `anim::validate` finds, one finding per code and bone (per file for `end-before-last-key`):

| Code | Kind | Meaning | Files on US EP4.5 |
|---|---|---|---|
| `parent-order` | error | a parent that is not -1 and not a bone before this one | 0 |
| `keys-not-increasing` | error | the keys of a track do not strictly increase | 0 |
| `end-before-last-key` | error | the end key is below the last key of any track | 0 |
| `key-not-finite` | error | a quaternion or translation component is NaN or infinite | 0 |
| `key-before-start` | warning | a key is below the first field | 0 |
| `track-starts-late` | warning | a track's first key is after key 0 | 382 tracks (all rotation tracks) in 254 files |
| `keys-close` | warning | two neighbouring keys are less than 2 apart | 0 |
| `quat-not-unit` | warning | a rotation key's length differs from 1 by more than 0.001 | not counted here |
| `matrix-not-finite` | warning | a reference matrix holds NaN or infinity | 20 |

A negative key is not an error: 269 tracks start at key -1, all in the 7 files whose first field is -1.

## Rig check

Between a candidate and the file it replaces, `rig_check` compares the bone count, every parent and every reference matrix as raw bits (the skeleton), and separately the first field. A candidate passes when it has the same skeleton, the same first field (inferred to be the start key; not checked in the game) and no validation error.

## How the toolkit poses a mesh

The pose is the toolkit's own statement of the math, an approximation of the game's playback, validated offline (nothing here was checked in the game). Matrices are 4 x 4, stored row by row, used with row vectors (`v' = v * M`), with the translation in the last row; `A * B` means "A, then B".

- A rotation key `(x, y, z, w)` becomes a rotation matrix in that row-vector form (a quarter turn about y takes `(1, 0, 0)` to `(0, 0, -1)`).
- `Local` of a root bone is its reference matrix; `Local` of any other bone is its reference matrix times the inverse of its parent's (a general 4 x 4 inverse; a singular or non-finite matrix gives NaN and the bone is counted).
- At time `t` (in key units) a bone's `Local` gets the sampled rotation in its upper 3 x 3 and the sampled translation in its last row, when it has such keys. Before a track's first key its first value holds and at or after its last key its last value holds; in between, rotations are slerped the short way and translations are linear. A bone's world matrix is its `Local` times its parent's world matrix.
- A `.3DC` vertex with weight `w` and bones `b1`, `b2` moves with `w * (B[b1] * World[b1]) + (1 - w) * (B[b2] * World[b2])`, where `B` are the mesh's own matrices. A vertex that names a bone the animation (or the mesh) does not have keeps its rest position and is counted.

Only plain interpolation is modelled; the game may differ.

On US EP4.5 the first rotation key of 123,201 of the 124,089 tracks that have one (99.28 %) matches the `Local` matrix of its bone within 0.001 (measured by an opt-in test with the shipped code: f32 matrices, the inverse in f64; the match is on the upper 3 x 3).

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `anim` | parses, writes back in memory, validates, runs the rig check, names the kind and rig of an archive path and computes a pose (`locals`, `pose`) |
| Shaiya Toolkit, Archive tab | previews and plays an `.ANI` (below) |
| `shaiya anim` (command line) | `info` describes, `validate` checks and `compare` runs the rig check against the file a candidate replaces (below) |

No program writes an `.ANI` file.

### In the Archive tab

Selecting an `.ANI` shows its kind ("Character animation, rig huwm (Human)", or "Animation in Monster/ANI" for any other folder), "65 bones, end key 240 (8.0 s at 30 keys per second)" with the number of rotation and translation keys, a first field other than 0 labelled "inferred: start key" (kept as it is), one warning-coloured line per validation error code and a weak line counting the warnings.

Below that is a picture of the animation posed on a mesh, with the bones drawn over it as lines, and the controls **Play/Pause**, a slider over the keys ("key 120 of 240, 4.0 s"), **Loop**, **Bones**, **Mesh** and a collapsible Bones table (bone, parent, number of rotation and translation keys, first and last key). Time runs at 30 key units per second. The frames are prepared on a worker thread before playing: one per key unit, or one every few key units for an animation longer than 240 keys, never more than 241 frames. The picture is an approximation of the game's playback, validated offline.

The mesh is found by name, never from a list file: the `3DC` folder next to the `ANI` folder (any letter case), files whose name equals the animation's prefix or starts with the prefix and an underscore. For `Character/<race>/ANI/<rig>_...` the prefix is the rig (`huwm`); for any other animation it is the file name without its last `_` part (`Mob_Bear_01_Att1` finds `Mob_Bear_01_A.3DC` but not `Mob_Bear_010_A.3DC`). A mesh is offered only when every bone its vertices use exists in the animation. The default is the first torso, lower, hand, boots, face and hair part of a character, and every compatible mesh (up to 8) for anything else; the Mesh box also offers "Skeleton only" and each mesh alone. Without a compatible mesh it says "no matching .3DC in <folder>; showing the skeleton" and draws the bones only. A vertex that names a bone the animation lacks is drawn unposed and counted. A file that does not read to the last byte shows the reader's error and the bytes as hex.

On US EP4.5, the default parts of each of the 16 character rigs, posed with that rig's first `<rig>_000_` animation, leave no vertex unposed (opt-in test, validated offline; the eight other matrix clients have their index only).

### On the command line

`shaiya anim info` prints one line per file (kind and rig of an archive path, bones, end key and seconds, key counts, errors and warnings; a first field other than 0 as "inferred: start key"). `shaiya anim validate` prints each finding with its code and exits 1 when a file has an error. `shaiya anim compare CANDIDATE TARGET` prints "same skeleton as TARGET" or each skeleton difference, the first field on its own line, the candidate's validation errors and `passes` or `does not pass` (exit 0 or 1); it passes under the rig-check rule above. Folders are searched for `.ani` and `.3dc` in any letter case. Details and JSON: [shaiya anim](../tools/shaiya.md#shaiya-anim).

## Verified on

US EP4.5 (patch 108), read only: all 3,509 `.ANI` files read to the last byte, write back byte for byte and have no validation error; 382 tracks (all rotation tracks) in 254 files start after key 0; 20 files hold NaN in reference matrices; the first field is -1 in 7 files, exactly those whose tracks start at key -1, and 0 in all others (inferred: the start key). The 16 character rigs have 1,192 files and 1,116 distinct skeletons (parents and reference matrices) between them; the lowest rig is 86 % distinct. Two animations that were replaced in that client pass the rig check against the stock files (2 files; compared with the original archive that sits beside the client).

## Other episodes

The eight other matrix clients have their index only: their `.ANI` files are counted (3,429 to 5,951), none was read. Whether they read is not checked.
