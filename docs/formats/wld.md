# The client map (`.wld`)

`world/<id>.wld` is the client's file for one map: terrain, placed models, sounds, portal boxes, named areas, bind (zone) groups and the NPC positions used for the minimap and quest marks. The game opens it by map number. `shaiya-clientdata` reads it (`Wld`, and `WldRaw` for lossless work). Shaiya SVMap Editor writes edited copies to a staging folder.

All numbers are little-endian. All 83 `.wld` files of the US EP4.5 client parse to the last byte: 29 fields (18 of 1024, 11 of 2048) and 54 dungeons.

## Conventions

- **name** is a fixed buffer of 256 bytes, NUL-terminated, in the client's code page (cp949 on EP4). The bytes after the NUL are leftovers from the map tool and mean nothing. A lossless writer keeps them.
- **List(R)** is `i32 nNames`, `name[nNames]`, `i32 n`, `R[n]`. Each record points at a name by its **index**, so a name must never be removed or moved: later indices would shift. An index of -1 means skip.
- **box** is `f32` min x, y, z and max x, y, z (24 bytes).
- **Placement** is `i32 name index`, `f32 pos[3]`, `f32 dir[3]`, `f32 up[3]` (40 bytes). `dir` and `up` give the rotation only. There is no scale: every building, shape and tree placement in the 83 files has unit, perpendicular `dir` and `up`.

## Layout

| Part | Layout |
|---|---|
| type | 4 bytes: `FLD` and a NUL for a field, `DUN` and a NUL for a dungeon |
| field terrain | `i32 mapSize` (even, 1024 or 2048 in the clients seen; the reader accepts up to 8192), then `i16 height[n * n]` and `u8 texIndex[n * n]` with `n = mapSize / 2 + 1`, index `cz * n + cx` |
| terrain textures | `i32 count` (at most 256), then count x {name texture, `f32 tiling`, name footstep} (516 bytes each) |
| water | name (a `.wtr` file) |
| dungeon | name of the `.dg` file ([Dungeon floors](dg.md)). Its geometry replaces the terrain. |
| List(Placement) x 7 | buildings, shapes, trees, grass, vani, vani2, dungeon pieces, in this order |
| mani | `i32 nNames`, names, `i32 n`, then n x {`i32`, `i32`, `f32 pos[3]`, `f32 dir[3]`, `f32 up[3]`} (44 bytes) |
| effect file | name |
| effects | `i32 n`, then n x {`f32 pos[3]`, `f32 dir[3]`, `f32 up[3]`, `i32 sequence`} (40 bytes) |
| objects | names, `i32 n`, n x object (76 bytes: `i32 index`, 9 `f32`, 9 `u32`), `i32 nPairs`, nPairs x two objects. Both lists are empty in every EP4 map. |
| List(Placement) | object2 |
| music | `i32 nNames`, names, `i32 n`, then n x {box, `f32 rotY`, `i32 bgmIndex`, `i32 flag`} (36 bytes) |
| ambient sound zones | `i32 nNames`, names, `i32 n`, then n x {box, `i32 k`, k x `i32 wavIndex`} |
| sound points | `i32 n`, then n x {`i32 wavIndex`, `f32 pos[3]`, `f32 radius`} (20 bytes) |
| oriented boxes | `i32 n`, then n x {box, `f32 rotY`} (28 bytes) |
| portals | `i32 n`, then n x 556 bytes: box, `f32 rotY`, name, 248-byte destination name (shown in the UI), `i32 country`, `i16 level min`, `i16 level max`, `i32 destMap`, `f32 destPos[3]` |
| zone groups | `i32 total`, then groups until `total` boxes were read: `i32 k`, then k x {box, `f32 rotY`, `i32 country`, `i32 attr`} (36 bytes). These are the server's bind areas. |
| areas | `i32 n`, then n x 548 bytes: box, `f32 rotY`, name, name of a caption `.tga`, `i32 mode`, `i32` |
| NPCs | `i32 n`, then records: `i32 type`, `i32 type id`, `f32 pos[3]`, `f32 angle`, `i32 k`, k x `f32 pos[3]` |
| sky (fields only) | three names: the sky dome `.bmp` and two cloud `.tga` files |
| colours and fog | three `f32[3]` colours, then `f32 fogNear`, `f32 fogFar` |

The file must end exactly after the fog values.

## The two counts that are not plain lengths

- **NPC list.** The client reads records while the number of records read is below `n`, and it subtracts each record's `k` extra points from `n` after reading it. So `n` equals the number of records plus all their extra points. A server NPC with m points is a client record with `k = m - 1`.
- **Zone groups.** `total` is the number of boxes of all groups, not the number of groups. A group with `k = 0` would make the client loop for ever, and the toolkit refuses it.

A writer must recompute both from the lists. A wrong value makes the client read garbage or crash on load.

## Terrain

The height of a corner is `i16 * 0.02 - 200` world units. Corners are 2 units apart. The game treats a height below -0.4 as water. The US EP4.5 client forces the size 1600 on map id 70 in memory.

## Area names

Area i is named by the line of `world/<id>.txt` whose first column is i ([area names](area-txt.md)). Mode 1 is a text caption and mode 2 a picture caption (`<id>_NN.tga`).

## Why the counts must be exact

The client reads a `.wld` with no length check. A file that is too short inside `data.saf` reads on into the next file, and the game shows garbage or crashes. So the writer counts every list exactly and refuses a model that would not read back (`WldRaw::validate`): a terrain grid of the wrong size, more than 256 terrain textures, a name list over 100,000, an empty zone group, and a few more.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` | `Wld` reads it into decoded names. `WldRaw` keeps every name as its raw 256 bytes and every value as read, parses and writes it back byte for byte, and recomputes the two counts above. |
| Shaiya SVMap Editor | reads it for terrain, portals, NPCs, regions and bind areas. It writes edited copies, only when the file writes back identically, to the staging folder as `data/world/<id>.wld`. |
| `shaiya svmap wld-check` | parses and writes back every `world/*.wld` of a client in memory and names the first byte that differs. |
| Shaiya Toolkit | The Archive tab previews `world/<id>.wld` as a grey top view of the terrain: one pixel per corner, north at the top, lit from the north-west, with "Map <id>, <n> x <n> corners, <size> units" and the lowest and highest height. A map larger than 4096 corners a side is shown shrunk. A dungeon map has no terrain, so the preview names its `.dg` file ("Dungeon map: the floor is in <name>; no top view yet"). A file that does not parse shows its error and the first bytes as hex. |

## Verified on

US EP4.5: all 83 files parse to the last byte and write back byte for byte (the version matrix rows `Wld::parse` and `wld_raw`). The map preview data builds for all 83 (the version matrix row "map preview data"). A game edit was checked live: a patch with a moved portal box was loaded by the game and used the new box.

## Other episodes

Other episodes are unverified. Each file is checked on its own: a file that does not write back identically stays read-only on the client side in the editor, and `shaiya svmap wld-check` names it (exit code 1, or 3 for a file that does not parse). The names use a code page, set in the editor's Preferences: cp949, Shift-JIS, GBK or Windows-1252. The test clients of the version matrix hold no world files, so those rows say `n/a`.
