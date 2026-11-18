# `TacticsZone.zon`

`world/TacticsZone.zon` is a client file that lists areas for the tactics (safe zone and dungeon room) features. The toolkit reads only the map id and the type of each area. The layout is inferred from the client's file, so a file that does not fit it is not judged.

## Layout as read

All numbers are little-endian.

| Part | Layout |
|---|---|
| header | `i32 version` (not checked), `i32 count` (at most 100,000) |
| each area | `u8` (not used), `u8 type`, `u8 sub type`, 4 corners of `f32 x, y, z` (48 bytes), 2 `f32` (a y range), `u16 map`, `i32 length`, then `length` name bytes (cp949, not used) |

The areas must end exactly at the last byte of the file. Type 1 is a safe zone. An area with type 100 is the file's placeholder on map 10000.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-svmap-tools` `cross_rules` | `parse_tactics_zones`, then the info rule `tactics-zone-map` for an area on a map that does not exist (the placeholder on map 10000 is skipped) |

The file is only read.

## Verified on

US EP4.5: the client's file (35 areas) parses to its last byte under this layout.

## Other episodes

Other episodes are unverified. A file that does not end exactly at its last byte is not checked at all, and no finding is made.
