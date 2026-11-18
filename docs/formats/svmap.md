# The server map (`.svmap`)

A `.svmap` file is the server's description of one map: its size, which cells block walking, the monster spawn boxes, the NPCs, the portals, the respawn (bind) areas and the named regions. The server keeps one per map, named by map number, in its map folder (for example `PS_Game\Data\Map\35.svmap`). The `shaiya-svmap` crate reads, writes, checks and compares it. Shaiya SVMap Editor and `shaiya svmap` use that crate.

All numbers are little-endian. A file is nine sections in this order, with no padding and no header. A reader must consume the file to its last byte.

## Layout

| # | Section | Layout | What the server does with it |
|---|---|---|---|
| 0 | size | `u32` | The side of the map in cells. A position is valid when `0 <= x, z < size`. |
| 1 | blocked cells | `ceil(size * size / 8)` bytes | One bit per cell: bit number `z * size + x`, least significant bit first, 1 = blocked. Monsters and guards do not walk on blocked cells. Players are not checked. |
| 2 | section A | `u32 n`, then n x {`f32 x, y, z`, `u32`, `u32`} (20 bytes) | Unknown. Empty in all 77 original files. |
| 3 | section B | `u32 n`, then n x {`f32 x, y, z`} | Unknown (the community calls them ladders). 0 to 8 points on 15 original maps. |
| 4 | monster areas | `u32 n`, then n x {`f32` min x, y, z, `f32` max x, y, z, `u32 m`, m x {`u32 mob`, `u32 count`}} | Spawn boxes. `mob` is the record index in `Monster.SData`. |
| 5 | NPCs | `u32 n`, then n x {`u32 type`, `u32 type id`, `u32 m`, m x {`f32 x, y, z`, `f32 dir`}} | NPC spawns. The first point is where the NPC stands, more points are a walk path. `dir` is in radians. |
| 6 | portals | `u32 n`, then n x 36 bytes {`f32 x, y, z`, `u32 country`, `u16 level min`, `u16 level max`, `u32 map`, `f32 x, y, z`} | The client sends a portal's index. The server checks that the player is within 40 units (x and z) of that index's position, and the country and levels. |
| 7 | bind areas | `u32 n`, then n x {`u32 m`, `u32 country`, `u32 attr`, m x box} | Respawn areas. A box is `f32` min x, y, z and max x, y, z (24 bytes). |
| 8 | regions | `u32 n`, then n x 32 bytes {box, `u32 kind`, `u8 b`, `u8 c`, `u16 d`} | Named areas. The server does not read them further. The client shows the name. |

Notes:

- The bind area record puts its box count first, then country and attr, then the boxes.
- A count the file cannot hold is not rejected up front. The records fail one by one with their number, and a count above 1,000,000 is refused before anything is allocated.
- The reader keeps the padding bits of the last blocked byte as they are, so a file written back is identical even when those bits are not zero.
- The writer copies every float bit for bit. Reading and writing an unchanged file gives the same bytes.

## What the original files hold

Measured on the 77 original server maps: sizes 1024 or 2048 for fields and 96 to 1024 for dungeons; fields are almost free of blocked cells and dungeons are 40 to 98 percent blocked. Monster areas are 2.0 units high in nearly every case, hold 0 to 4 entries with counts 1 to 8, and 61 are empty. NPC types are 0 to 12, with 1 to 7 points. Portal countries are 0, 1, 2 and the instance ids from 1001. Map 200 is the guild ranking and has no file. Bind areas have one box each and attr 0. Regions end in tails such as (0, 0, 0, 0) or (2, 0, 0, 0).

## How it mirrors the client map

The client's `world/<id>.wld` ([World file](wld.md)) holds a second copy of some records. The two must agree:

| Server section | Client record | Rule |
|---|---|---|
| NPCs | NPC list | The same list, record for record: type, type id, first point, angle, path points. |
| Portals | portal boxes | The same order and fields. The server position is the box centre (x, z) and the box's lowest y. The index matters because the client sends it. |
| Regions | named areas | The same boxes in the same order. The server `kind` equals the client area's mode. The name is line i of `world/<id>.txt` ([area names](area-txt.md)). |
| Bind areas | zone groups | One group per bind area, one box each, in the same order. |
| Monster areas, sections A and B, blocked cells | none | They exist only on the server. |

In the original maps the counts differ for the NPCs of maps 19, 20, 28, 29 and 40, the portals of maps 6, 8, 35 and 36, and the regions of seven maps (19, 28, 35, 36, 47, 50 and 104).

## JSON

`shaiya svmap dump --json` and the editor's Export > Map JSON write the same document, and `shaiya svmap build` reads it back. It is a JSON object with these keys:

| Key | Value |
|---|---|
| `size` | the number |
| `blocked` | the packed blocked bytes as base64 (standard alphabet, with padding) |
| `section_a` | a list of `{"pos": [x, y, z], "a": n, "b": n}` |
| `points_b` | a list of `[x, y, z]` |
| `mob_areas` | a list of `{"min": [x, y, z], "max": [x, y, z], "mobs": [{"mob": n, "count": n}]}` |
| `npcs` | a list of `{"kind": n, "type_id": n, "points": [{"pos": [x, y, z], "dir": f}]}` |
| `portals` | a list of `{"pos": [x, y, z], "country": n, "level_min": n, "level_max": n, "to_map": n, "to_pos": [x, y, z]}` |
| `binds` | a list of `{"country": n, "attr": n, "boxes": [{"min": [x, y, z], "max": [x, y, z]}]}` |
| `regions` | a list of `{"min": [x, y, z], "max": [x, y, z], "kind": n, "b": n, "c": n, "d": n}` |

Floats are JSON numbers: the shortest decimal that reads back to the same `f32`. JSON cannot carry NaN or infinity, so a map that holds one cannot be dumped. `build` refuses a `blocked` string that is not base64 or whose decoded length is not `ceil(size * size / 8)`.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-svmap` (library) | read, write, JSON, validation (the rules `pos-out-of-range`, `box-inverted`, `area-empty` and the others of the editor's rule list), diff, bit grid operations |
| Shaiya SVMap Editor | reads and writes the file; every save writes a temporary file, reads it back, keeps a `.bak` of the first version and then replaces the file |
| `shaiya svmap` | `check`, `dump`, `build`, `roundtrip`, `diff`, `spawns`, `bulk`; only `build` writes, and only to the `-o` file |

## Verified on

The 77 original server map files (US EP4.5) all parse to the last byte and write back byte for byte, both as binary and through JSON. Afterglow's maps use the same layout.

## Other episodes

The reader is strict: a file that does not end at its last byte is refused (an error in the editor, exit code 3 in `shaiya svmap`). Maps of EP3 and EP5.4 servers are reported to share the layout, but that is unverified. Other server builds are unverified.
