# Formats: overview

Every file format the toolkit reads or writes has a page here. A page says what the format is, which program touches it, and where it was **verified**. The pages state facts about the files, not addresses or internals of the game, and they say so when a fact was checked on one client only. A format that was checked on a client but whose table loader was not read there is called a fit, never verified; the [version matrix](../version-matrix.md) has the measured rows.

"Verified" below means what the page's own "Verified on" paragraph says. "US EP4.5" is the client the author used as the reference (the `us-108` column of the matrix). Where the matrix test clients are named (`ru-ep3`, `ru-ep45`, `ru-ep54`, `fr-ep54`, `phoenix-ep63`, `kr-ep8`, `fawkes-v68`, `cn-ep4`), the numbers are measured rows.

## Documented formats

| Format | Page | Read by | Written by | Verified on |
|---|---|---|---|---|
| Archive (`data.sah`, `data.saf`) | [archive.md](archive.md) | `shaiya-archive`: the Archive tab, `shaiya archive`, patch apply, `shaiya info` | in-place edits, `archive compact`, `archive swap`, patch apply; a new pair for the `-o` forms | the matrix rows `Index::parse` and `to_bytes` byte-identical on all nine clients (the `fawkes-v68` index is version 1; its data end is past 4 GiB) |
| Patch (`psNNNN.patch`) | [patch.md](patch.md) | the Patch builder, `shaiya patch list|verify|apply`, the Apply tab | `shaiya patch build`, the Patch builder, "Make patch from differences", Shaiya SVMap Editor | synthetic patches through apply and rollback on a copy of each client's real index, all nine clients (matrix row `patch_apply_rollback_stub`); the original updater's handling is on the Version.ini page |
| `Version.ini` and `UpdateVersion.ini` | [version-ini.md](version-ini.md) | Shaiya Toolkit and `shaiya clients show` read the client's `Version.ini`; Publish reads the existing `UpdateVersion.ini` | `shaiya patch publish` and the Patch builder's Publish section write `UpdateVersion.ini` | the author's analysis of the original US EP4.5 updater; other episodes not verified |
| SData wrapper (SEED, CRC) | [sdata-wrapper.md](sdata-wrapper.md) | `shaiya-seed`: `shaiya sdata`, the SData tab, the checks before files go into an archive or a patch | `shaiya sdata decrypt|encrypt` (to a folder, or in place with a `.bak`); the SData tab's Decrypt and Encrypt to folder | every SData of every matrix client that has the file, wrap(unwrap(x)) byte for byte (`cn-ep4` stores them plain) |
| `Item.SData` | [item-sdata.md](item-sdata.md) | `shaiya-sdata`: `shaiya sdata table`, the SData tab's View table | not written (read only) | verified on `us-108`, `ru-ep3`, `ru-ep45`, `fr-ep54` (layout `item-81`) and `phoenix-ep63` (`item-132`); FIT-ONLY (reads the file, no loader read) on `cn-ep4` (`item-81`), `kr-ep8` and `fawkes-v68` (`item-134`) |
| `Skill.SData` | [skill-sdata.md](skill-sdata.md) | `shaiya-sdata`, as above | not written (read only) | verified on `us-108`, `ru-ep3`, `ru-ep45` (`skill-3`), `fr-ep54` (`skill-9`), `phoenix-ep63` (`skill-15`); FIT-ONLY on `cn-ep4`, `kr-ep8`, `fawkes-v68`; the monster skill file was measured, not verified |
| `Monster.SData` | [monster.md](monster.md) | `shaiya-clientdata`, `shaiya-sdata`; Shaiya SVMap Editor for monster ids | not written (read only) | US EP4.5 (2,474 records); the table layout on `ru-ep3`, `ru-ep45`, `fr-ep54`, `phoenix-ep63`; FIT-ONLY on `kr-ep8`, `fawkes-v68` (`monster-36`); no fit on `cn-ep4` |
| `NpcQuest.SData` | [npcquest.md](npcquest.md) | `shaiya-clientdata`, `shaiya-sdata`; Shaiya SVMap Editor and `shaiya svmap check` | not written (read only) | US EP4.5; also reads on `ru-ep3`, `ru-ep45`, `fr-ep54`, `phoenix-ep63`, `cn-ep4`; the table layouts are FIT-ONLY on `kr-ep8` and `fawkes-v68` (`npcquest-287`) and on `cn-ep4` (`npcquest-165`) |
| Server map (`.svmap`) | [svmap.md](svmap.md) | `shaiya-svmap`: Shaiya SVMap Editor, `shaiya svmap` | the editor's save, `shaiya svmap build` | the 77 original US EP4.5 server maps, byte for byte, as binary and through JSON |
| Client map (`.wld`) | [wld.md](wld.md) | `shaiya-clientdata`: Shaiya SVMap Editor | the editor, to a staging folder | US EP4.5: all 83 files, byte for byte |
| World area names (`world/<id>.txt`) | [area-txt.md](area-txt.md) | `shaiya-clientdata`: the editor | the editor, to a staging folder | US EP4.5: 33 files, byte for byte |
| Server `Map.ini`, `MapWar.ini` | [map-ini.md](map-ini.md) | the editor, `shaiya svmap check --server-data` | the editor (line-preserving edits) | the original US EP4.5 server files, byte for byte |
| Server `Obelisk.ini` | [obelisk-ini.md](obelisk-ini.md) | the editor, `shaiya svmap check --server-data` | not written | the original US EP4.5 server file |
| Dungeon floors (`.dg`) | [dg.md](dg.md) | `shaiya-clientdata`: the editor | not written | US EP4.5: 44 files to the last byte |
| Models (`.smod`) | [smod.md](smod.md) | `shaiya-clientdata`: the editor | not written | US EP4.5: 1,355 of 1,355 to the last byte |
| Animations (`.ANI`) | [ani.md](ani.md) | `shaiya-clientdata`: `Ani::parse`, `validate`, `rig_check`; Shaiya Toolkit's Archive tab previews and plays it | not written by any tool; `Ani::to_bytes` builds bytes in memory for the tests | US EP4.5: 3,509 of 3,509 read to the last byte and write back byte for byte; the eight other matrix clients are index only (files counted, none read) |
| Skinned meshes (`.3DC`) | [3dc.md](3dc.md) | `shaiya-clientdata`: `C3dc::parse`; Shaiya Toolkit's Archive tab previews it at rest and uses it as an animation's mesh | not written by any tool; `C3dc::to_bytes` builds bytes in memory for the tests | US EP4.5: 2,431 of 2,431 read to the last byte and write back byte for byte; the eight other matrix clients are index only (files counted, none read) |
| Vertex-animated models (`.VANI`) | [vani.md](vani.md) | `shaiya-clientdata`: `Vani::parse`; Shaiya Toolkit's Archive tab previews and plays it, `shaiya anim info` describes it | not written by any tool; `Vani::to_bytes` builds bytes in memory for the tests | US EP4.5: 66 of 66 read to the last byte and write back byte for byte; the eight other matrix clients are index only (files counted, none read) |
| Building motion (`.MANI`) | [mani.md](mani.md) | `shaiya-clientdata`: `Mani::parse`; Shaiya Toolkit's Archive tab shows its fields, `shaiya anim info` describes it; every field meaning is inferred | not written by any tool; `Mani::to_bytes` builds bytes in memory for the tests | US EP4.5: 23 of 23 (108 bytes each) read and write back byte for byte; the nine indexes list only 108-byte files; the eight other matrix clients are index only (files counted, none read) |
| `.3DO` meshes | [3do.md](3do.md) | `shaiya-clientdata`: `Mesh3do::parse`; Shaiya Toolkit's Archive tab previews it (the one file with frames plays), `shaiya anim info` describes it; the frame rate is inferred | not written by any tool; `Mesh3do::to_bytes` builds bytes in memory for the tests | US EP4.5: 271 of 271 read to the last byte and write back byte for byte (one file has frames); the eight other matrix clients are index only (files counted, none read) |
| Effect meshes (`.3DE`) | [3de.md](3de.md) | `shaiya-clientdata`: `Mesh3de::parse`; Shaiya Toolkit's Archive tab previews it (frames are listed, not played), `shaiya anim info` describes it; two layouts, the frame fields are inferred | not written by any tool; `Mesh3de::to_bytes` builds bytes in memory for the tests | US EP4.5: 467 of 469 read to the last byte and write back byte for byte, 2 are refused (named on the page); the eight other matrix clients are index only (files counted, none read) |
| Textures (DDS, TGA) | [textures.md](textures.md) | `shaiya-clientdata`, the Archive tab's preview, the checks before files go in | the editor writes light maps, masks and a minimap for new maps | US EP4.5; the orientation of the minimap and the chunk layout of a blank field were not checked in the game |
| `sysmsg-uni.txt` and map names | [sysmsg.md](sysmsg.md) | `shaiya-clientdata`: the editor, `shaiya svmap` | not written | US EP4.5 patch 108; text for 47 of 47 ids on every matrix client that has the file, 44 on `cn-ep4` |
| `TacticsZone.zon` | [tacticszone-zon.md](tacticszone-zon.md) | `shaiya-clientdata`: the editor | not written | US EP4.5: 35 areas to the last byte |
| Afterglow `mobs.psv` | [mobs-psv.md](mobs-psv.md) | the editor, `shaiya svmap check --afterglow-defs` | not written | an Afterglow export; only the id column |
| Files Shaiya SVMap Editor defines | [svmap-editor-files.md](svmap-editor-files.md) | the editor (its install record only) | the editor | tests, and a copy of a US EP4.5 test client for the install record |
| The toolkit's own files in a client folder | [toolkit-files.md](toolkit-files.md) | the toolkit's recovery, rollback and Put back | the toolkit's apply, compact, swap and clean-up | the toolkit's tests and code; the matrix rollback rows |

## Known, not yet documented

These extensions are in the archives of the nine matrix clients and no page covers them. The numbers are the visible files each archive's index lists with that extension, measured read-only with `shaiya archive stats`: the count in `us-108`, the lowest and highest count over the nine clients, and how many of the nine have any. The toolkit lists these files, extracts and replaces them like any archive file, and shows what it cannot decode as hex in the Archive tab ([the toolkit page](../tools/toolkit.md)); it reads none of them as a format. "Planned for" is a plan, not a promise.

| Extension | `us-108` | Range over nine clients | Clients with any | Planned for |
|---|---|---|---|---|
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

Two extensions are only partly covered. `.TXT` (35 files in `us-108`, 33 to 567): `world/<id>.txt` is [area-txt.md](area-txt.md) and `sysmsg-uni.txt` is [sysmsg.md](sysmsg.md); no other text file has a page. `.SData` (8 files in `us-108`, 8 to 71): the four tables above and the [wrapper](sdata-wrapper.md) are documented; every other table is shown as hex, except NpcSkill, which a skill layout reads ([version matrix](../version-matrix.md)).

The extensions that have a page are `.DDS` and `.TGA` ([textures.md](textures.md)), `.SMOD`, `.WLD`, `.DG` and `.ZON` (first table). `.SVMAP` is the server's file ([svmap.md](svmap.md)); four matrix clients also hold some in their archives (56, 5, 98 and 16 files).

A format moves from this table to the first one when it has a page with a "Verified on" paragraph and the toolkit reads it. [Editing these docs](../dev/docs-site.md) says how pages are added.
