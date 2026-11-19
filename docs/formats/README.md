# Formats: overview

Every file format the toolkit reads or writes has a page here. A page says what the format is, which program touches it, and where it was **verified**. The pages state facts about the files, not addresses or internals of the game, and they say so when a fact was checked on one client only. A format that was checked on a client but whose table loader was not read there is called a fit, never verified; the [version matrix](../version-matrix.md) has the measured rows.

"Verified" below means what the page's own "Verified on" paragraph says. "US EP4.5" is the client the author used as the reference (the `us-108` column of the matrix). Where the matrix test clients are named (`ru-ep3`, `ru-ep45`, `fr-ep54`, `phoenix-ep63`, `kr-ep8`, `fawkes-v68`, `cn-ep4`), the numbers are measured rows.

## Documented formats

| Format | Page | Read by | Written by | Verified on |
|---|---|---|---|---|
| Archive (`data.sah`, `data.saf`) | [archive.md](archive.md) | `shaiya-archive`: the Archive tab, `shaiya archive`, patch apply, `shaiya info` | in-place edits, `archive compact`, `archive swap`, patch apply; a new pair for the `-o` forms | the matrix rows `Index::parse` and `to_bytes` byte-identical on all nine clients (the `fawkes-v68` index is version 1; its data end is past 4 GiB) |
| Patch (`psNNNN.patch`) | [patch.md](patch.md) | the Patch builder, `shaiya patch list|verify|apply`, the Apply tab | `shaiya patch build`, the Patch builder, "Make patch from differences", Shaiya SVMap Editor | synthetic patches through apply and rollback on a copy of each client's real index, all nine clients (matrix row `patch_apply_rollback_stub`); the original updater's handling is on the Version.ini page |
| `Version.ini` and `UpdateVersion.ini` | [version-ini.md](version-ini.md) | Shaiya Toolkit reads the client's `Version.ini` | `shaiya patch publish` and the Patch builder's Publish section write `UpdateVersion.ini` | the original US EP4.5 updater; other episodes not verified |
| SData wrapper (SEED, CRC) | [sdata-wrapper.md](sdata-wrapper.md) | `shaiya-seed`: `shaiya sdata`, the SData tab, the checks before files go into an archive or a patch | `shaiya sdata decrypt|encrypt` (to a folder, or in place with a `.bak`) | every SData of every matrix client that has the file, wrap(unwrap(x)) byte for byte (`cn-ep4` stores them plain) |
| `Item.SData` | [item-sdata.md](item-sdata.md) | `shaiya-sdata`: `shaiya sdata table`, the SData tab's View table | not written (read only) | verified on `us-108`, `ru-ep3`, `ru-ep45`, `fr-ep54` (layout `item-81`) and `phoenix-ep63` (`item-132`); `cn-ep4` fits only; `kr-ep8` and `fawkes-v68` fit no layout |
| `Skill.SData` | [skill-sdata.md](skill-sdata.md) | `shaiya-sdata`, as above | not written (read only) | verified on `us-108`, `ru-ep3`, `ru-ep45` (`skill-3`), `fr-ep54` (`skill-9`), `phoenix-ep63` (`skill-15`); `cn-ep4`, `kr-ep8`, `fawkes-v68` fit only; the monster skill file was measured, not verified |
| `Monster.SData` | [monster.md](monster.md) | `shaiya-clientdata`, `shaiya-sdata`; Shaiya SVMap Editor for monster ids | not written (read only) | US EP4.5 (2,474 records); the table layout on `ru-ep3`, `ru-ep45`, `fr-ep54`, `phoenix-ep63` |
| `NpcQuest.SData` | [npcquest.md](npcquest.md) | `shaiya-clientdata`, `shaiya-sdata`; Shaiya SVMap Editor and `shaiya svmap check` | not written (read only) | US EP4.5; also reads on `ru-ep3`, `ru-ep45`, `fr-ep54`, `phoenix-ep63`, `cn-ep4` |
| Server map (`.svmap`) | [svmap.md](svmap.md) | `shaiya-svmap`: Shaiya SVMap Editor, `shaiya svmap` | the editor's save, `shaiya svmap build` | the 77 original US EP4.5 server maps, byte for byte, as binary and through JSON |
| Client map (`.wld`) | [wld.md](wld.md) | `shaiya-clientdata`: Shaiya SVMap Editor | the editor, to a staging folder | US EP4.5: all 83 files, byte for byte |
| World area names (`world/<id>.txt`) | [area-txt.md](area-txt.md) | `shaiya-clientdata`: the editor | the editor, to a staging folder | US EP4.5: 33 files, byte for byte |
| Server `Map.ini`, `MapWar.ini` | [map-ini.md](map-ini.md) | the editor, `shaiya svmap check --server-data` | the editor (line-preserving edits) | the original US EP4.5 server files, byte for byte |
| Server `Obelisk.ini` | [obelisk-ini.md](obelisk-ini.md) | the editor, `shaiya svmap check --server-data` | not written | the original US EP4.5 server file |
| Dungeon floors (`.dg`) | [dg.md](dg.md) | `shaiya-clientdata`: the editor | not written | US EP4.5: 44 files to the last byte |
| Models (`.smod`) | [smod.md](smod.md) | `shaiya-clientdata`: the editor | not written | US EP4.5: 1,355 of 1,355 to the last byte |
| Textures (DDS, TGA) | [textures.md](textures.md) | `shaiya-clientdata`, the Archive tab's preview, the checks before files go in | the editor writes light maps, masks and a minimap for new maps | US EP4.5; the orientation of the minimap and the chunk layout of a blank field were not checked in the game |
| `sysmsg-uni.txt` and map names | [sysmsg.md](sysmsg.md) | `shaiya-clientdata`: the editor, `shaiya svmap` | not written | US EP4.5 patch 108; text for 47 of 47 ids on every matrix client that has the file, 44 on `cn-ep4` |
| `TacticsZone.zon` | [tacticszone-zon.md](tacticszone-zon.md) | `shaiya-clientdata`: the editor | not written | US EP4.5: 35 areas to the last byte |
| Afterglow `mobs.psv` | [mobs-psv.md](mobs-psv.md) | the editor, `shaiya svmap check --afterglow-defs` | not written | an Afterglow export; only the id column |
| Files Shaiya SVMap Editor defines | [svmap-editor-files.md](svmap-editor-files.md) | the editor (its install record only) | the editor | tests, and a copy of a US EP4.5 test client for the install record |
| The toolkit's own files in a client folder | [toolkit-files.md](toolkit-files.md) | the toolkit's recovery and rollback | the toolkit's apply, compact and swap | the toolkit's tests and code; the matrix rollback rows |

## Known, not yet documented

These extensions are in the archives of the nine matrix clients and no page covers them. The numbers are the visible files each archive's index lists with that extension, measured read-only with `shaiya archive stats`: the count in `us-108`, the lowest and highest count over the nine clients, and how many of the nine have any. The toolkit lists these files, extracts and replaces them like any archive file, and shows what it cannot decode as hex in the Archive tab ([the toolkit page](../tools/toolkit.md)); it reads none of them as a format. "Planned for" is a plan, not a promise.

| Extension | `us-108` | Range over nine clients | Clients with any | Planned for |
|---|---|---|---|---|
| `.3DC` | 2,431 | 2,317 to 6,534 | 9 | 0.4.0 animations |
| `.3DO` | 271 | 271 to 514 | 9 | 0.4.0 animations |
| `.3DE` | 469 | 469 to 1,344 | 9 | 0.4.0 animations |
| `.ANI` | 3,509 | 3,429 to 5,951 | 9 | 0.4.0 animations |
| `.VANI` | 66 | 66 to 141 | 9 | 0.4.0 animations |
| `.MANI` | 23 | 23 to 64 | 9 | 0.4.0 animations |
| `.EFT` | 446 | 423 to 969 | 9 | 0.5.0 effects |
| `.SEFF` | 4 | 4 to 5 | 9 | 0.5.0 effects |
| `.FX` | 0 | 0 to 6 | 2 | 0.5.0 effects |
| `.MLT` | 112 | 112 to 134 | 9 | not planned |
| `.ITM` | 17 | 17 | 9 | not planned |
| `.MON` | 16 | 16 to 17 | 9 | not planned |
| `.ALT` | 16 | 16 to 19 | 9 | not planned |
| `.WAV` | 1,019 | 1,019 to 1,679 | 9 | not planned |
| `.BMP` | 29 | 24 to 209 | 9 | not planned |
| `.JPG` | 22 | 12 to 148 | 9 | not planned |
| `.ENV` | 0 | 0 to 139 | 2 | not planned |
| `.PC` | 128 | 128 | 9 | not planned |
| `.XML` | 0 | 0 to 88 | 3 | not planned |
| `.DB` | 0 | 0 to 42 | 8 | not planned |
| `.MLX` | 0 | 0 to 16 | 1 | not planned |
| `.DG_PV` | 0 | 0 to 13 | 7 | not planned |
| `.WTR` | 2 | 2 to 8 | 9 | not planned |
| `.CUR` | 7 | 7 | 9 | not planned |
| `.CTL` | 4 | 4 | 9 | not planned |
| `.INI` | 0 | 0 to 4 | 3 | not planned |
| `.DAT` | 3 | 3 | 9 | not planned |
| `.CFG` | 0 | 0 to 3 | 5 | not planned |
| `.3DC_` | 0 | 0 to 2 | 5 | not planned |
| `.TOL` | 0 | 0 to 2 | 7 | not planned |
| `.PNG` | 0 | 0 to 2 | 1 | not planned |
| `.EXE` | 0 | 0 to 1 | 8 | not planned |
| `.ZIP` | 0 | 0 to 1 | 3 | not planned |
| `.PSD` | 0 | 0 to 1 | 2 | not planned |
| `.EPS` | 0 | 0 to 1 | 1 | not planned |
| `.BAT` | 0 | 0 to 1 | 1 | not planned |

Two extensions are only partly covered. `.TXT` (35 files in `us-108`, 33 to 567): `world/<id>.txt` is [area-txt.md](area-txt.md) and `sysmsg-uni.txt` is [sysmsg.md](sysmsg.md); no other text file has a page. `.SData` (8 files in `us-108`, 8 to 71): the four tables above and the [wrapper](sdata-wrapper.md) are documented; every other table is shown as hex and has no layout planned yet.

The extensions that have a page are `.DDS` and `.TGA` ([textures.md](textures.md)), `.SMOD`, `.WLD`, `.DG` and `.ZON` (first table). `.SVMAP` is the server's file ([svmap.md](svmap.md)); four matrix clients also hold some in their archives (56, 5, 98 and 16 files).

A format moves from this table to the first one when it has a page with a "Verified on" paragraph and the toolkit reads it. [Editing these docs](../dev/docs-site.md) says how pages are added.
