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
  f32 reference matrix[16]       64 bytes, row-major, translation in elements 12 to 14
  i32 nRot,   nRot   x { i32 key; f32 x, y, z, w }    rotation keys (a quaternion)
  i32 nTrans, nTrans x { i32 key; f32 x, y, z }       translation keys
```

Key times are in units of 1/30 second (30 per second is the game's default rate). The file must end at the last byte. There are no bone names: meshes are matched to bones by index.

The toolkit keeps floats exactly as stored, NaN bits included, so a file written back is the same bytes as the file read. Reference matrices that hold NaN are kept bit for bit.

## Rules a replacement must keep

A replacement must keep the bone count, the order and parents of the bones and all 64 bytes of every reference matrix of the file it replaces. Almost every file of a rig has its own reference matrices, so copy them from the exact file you replace.

The toolkit also asks for the same first field as the file it replaces (inferred to be the start key; not checked in the game).

## What the toolkit checks (validation codes)

`shaiya-clientdata` `anim::validate` finds, one finding per code and bone:

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

Between a candidate and the file it replaces, `rig_check` compares the bone count, every parent and every reference matrix as raw bits (the skeleton), and separately the first field. A candidate passes when it has the same skeleton, the same first field and no validation error.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `anim` | parses, writes back in memory, validates, runs the rig check and names the kind and rig of an archive path |

No program shows or writes an `.ANI` yet: the Archive tab lists, extracts and replaces it like any file and shows it as hex.

## Verified on

US EP4.5 (patch 108), read only: all 3,509 `.ANI` files read to the last byte, write back byte for byte and have no validation error; 382 tracks (all rotation tracks) in 254 files start after key 0; 20 files hold NaN in reference matrices; the first field is -1 in 7 files, exactly those whose tracks start at key -1, and 0 in all others (inferred: the start key). The 16 character rigs have 1,192 files and 1,116 distinct skeletons (parents and reference matrices) between them; the lowest rig is 86 % distinct. Two animations that were replaced in that client pass the rig check against the stock files (2 files; compared with the original archive that sits beside the client).

## Other episodes

The eight other matrix clients have their index only: their `.ANI` files are counted (3,429 to 5,951), none was read. Whether they read is not checked.
