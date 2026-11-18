# Files the SVMap Editor defines

Most files Shaiya SVMap Editor touches are the game's or the server's own formats, described on their pages. This page covers the files the editor defines itself: the CSV export, the install record, the index backups, the new-map note file and the time stamp used in their names. The editor writes them. It reads back only the install record.

## The UTC stamp

Backup names and the entries of the install record carry the time in UTC as `YYYYMMDD-HHMMSSZ`, for example `20261113-140205Z`. When an index copy with the stamp exists already, `_1`, `_2` and so on are added. The editor parses the stamp back for its backup lists. Its log lines are `<UTC time> <level> <message>`, and `log.txt` rotates to `log.1.txt` at 1 MB.

## CSV export

**File > Export** writes one CSV file per section of the open map, all five into a folder, or the monster areas of every map in one file.

- UTF-8 with a byte order mark, CRLF line ends, a comma as the separator, one header row.
- A text cell that holds a comma, a double quote or a line break is wrapped in double quotes, with inner quotes doubled.
- A text cell that starts with `=`, `+`, `-`, `@`, a TAB or a carriage return gets a leading `'`, so a spreadsheet does not run it as a formula.
- Numbers use the shortest decimal that reads back to the same `f32`. Angles are in degrees with two decimals.
- File names are `<map>-<part>.csv` with the part `monster-areas`, `npcs`, `portals`, `binds` or `regions`.

| Section | Columns |
|---|---|
| monster areas | `index,x1,y1,z1,x2,y2,z2,width,depth,mob,mob_name,mob_level,count` (one row per monster entry, one row with empty monster columns for an empty area) |
| NPCs | `index,type,type_name,type_id,name,points,x,y,z,dir_deg` |
| portals | `index,x,y,z,country,level_min,level_max,to_map,to_map_name,to_x,to_y,to_z` |
| bind areas | `index,country,attr,box,x1,y1,z1,x2,y2,z2` (one row per box) |
| regions | `index,name,kind,b,c,d,x1,y1,z1,x2,y2,z2` |
| every map's monster areas | the columns of the monster areas with a `map` column first |

An export never writes a `.svmap`, `.sah` or `.saf` file, or a map file that is open. **Export > Map JSON** writes the document of [the svmap format](svmap.md#json).

## The install record `svmap-editor-install.json`

Install writes it into the test client folder, through a `.tmp` file and a rename, and updates it on every install. Uninstall reads it, checks it and removes it. It is pretty-printed JSON with these keys:

| Key | Content |
|---|---|
| `tool` | `svmap-editor` |
| `first_backup` | `data.sah.svmap-editor` |
| `sah_before` | `{"len": n, "hash": "<16 hex digits>"}` for `data.sah` before the first install |
| `saf_before` | `{"len": n, "head": "<hex>", "tail": "<hex>"}` for the first `len` bytes of `data.saf` before the first install |
| `sah_after`, `saf_after` | the same, after the latest install |
| `installs` | one entry per install: `{"time": "<UTC stamp>", "backup": "<file name of the stamped index copy>", "files": ["<archive path>", ...]}` |

A hash is FNV-1a with 64 bits (offset basis `0xcbf29ce484222325`, prime `0x100000001b3`) written as 16 lower-case hex digits. For `data.sah` it covers every byte. For `data.saf` it is not a full hash: `head` covers the first MiB (1,048,576 bytes) and `tail` the last MiB of the first `len` bytes (less when the file is shorter), together with the length. The edge hashes notice a game update or a tool that appended or cut the file. They would miss a rewrite in the middle of the old part that keeps the length, which is why the index entries are compared too.

Uninstall refuses when `data.sah` or the old part of `data.saf` no longer matches the record, because restoring would undo someone else's change.

## The index backups

- `data.sah.svmap-editor`: a copy of `data.sah` from before the first install. It is made once and never replaced while the record exists. An existing file of that name without a record is refused, so an old backup is never mistaken for this install's. Uninstall puts it back and removes it.
- `data.sah.svmap-editor-<UTC stamp>`: a copy of `data.sah` taken before each install, checked byte for byte. It stays after Uninstall.

`data.sah.bak` in the same folder is made by the patch step on every install and is overwritten each time. It is not a backup of this editor.

## The new-map note file

`svmap-editor-new-map-<id>.txt` is written by Add a map into the staging folder, outside `data` so a patch built from `data` does not carry it. It is plain text with CRLF line ends. It lists, in this order:

1. `New map <id> made by svmap-editor`
2. `Target server:` original or Afterglow
3. `Source:` a copy of a client map (field or dungeon, with its size) or a flat field and the map its textures, sky and fog came from
4. `Name you chose:` when you typed a name, and whether the client already has a name for the id. The name is not written into the game.
5. `Not done:` one line per thing the wizard cannot do (the map name, terrain textures, the loading picture `<id>.bmp`, music zones, rebirth positions, blocked cells of a dungeon, and for Afterglow the import script)

## Verified on

The CSV rules and the stamp are covered by tests. The install record, the first backup and Uninstall were checked on a copy of a US EP4.5 test client: the staged file read back from `data.saf`, `data.saf` grew by exactly the staged bytes, and Uninstall gave back `data.sah` and `data.saf` byte for byte.

## Other episodes

The files hold no episode data. Install needs a `data.sah` with a header, which all nine clients of the version matrix have. A `data.sah` without a header is refused.
