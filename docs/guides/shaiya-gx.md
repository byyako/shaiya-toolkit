# Shaiya-GX folders

[Shaiya-GX](https://github.com/Spelunkern/Shaiya-GX) is a separate project: a client and a server that read their game data from plain folders (EP5.4). This page says what Shaiya Toolkit does with folders laid out the way Shaiya-GX documents them, and what each statement rests on.

**What this page does not say.** No real Shaiya-GX install was opened, changed or run for this page. Every layout fact below was read from the Shaiya-GX documentation (cited as the file in their repository and its line, as it was read for this release). Every behaviour of the toolkit below was tested on a *synthetic* folder made by a test, or, where marked, on files of a US EP4.5 client copied into a new folder laid out the Shaiya-GX way. "Works with Shaiya-GX" is therefore not claimed: it is claimed only that the toolkit does these things on such folders, and the tests are named so you can read them.

The labels used in the tables:

- **synthetic tree**: a folder a test builds with made-up files in the layout described by the Shaiya-GX documentation (the test is named).
- **US files, GX layout (opt-in)**: files of one US EP4.5 client copied into a new folder with that layout; the test needs a client folder and a scratch folder (`SHAIYA_CLIENT`, `SHAIYA_SCRATCH`) and is not part of the default test run.
- **read from Shaiya-GX docs**: not checked by us at all.
- **not tested on a Shaiya-GX install**: true of every row.

## Before you change anything

- Stop the game and the server first. Shaiya-GX documents that its client reads the data folder once at start and that assets need a restart after editing (docs/data-formats.md line 18), so restart the game after a change or an undo (read from Shaiya-GX docs; not tried). For the server the toolkit knows nothing about when its files are read: stop it too.
- A Shaiya-GX `data` folder is not an extracted original client. Its images and sounds were converted (interface pictures and icons are PNG, `_dds.png` marks an atlas that was a DDS, audio is Ogg Vorbis; docs/data-formats.md lines 22-36 and 79-89), and the documentation says the original clients cannot use that layout without their own loader (line 73). The folder can hold the only copy of those converted files. The toolkit refuses to write them (see below) and never converts anything.
- Keep your own copy of anything you cannot download again. A change to a loose folder is undone by copying files back, and only while the files are as the change left them ([undo and recovery](undo-and-recovery.md), "Loose data folders").

## Client folder (`Client/data`)

A folder is taken as a loose client by the rules on [Loose data folders](../formats/loose-client.md). A Shaiya-GX folder is recognised there by a `.ogg` under `Sound/` or a `*_dds.png` file; that sign is read from the documentation lines above, not from a real folder.

| What | Works | Tested on |
|---|---|---|
| Add as a client, browse, search, filters | yes, read only until a backup folder outside the data folder is confirmed | synthetic tree: `a_loose_client_browses_and_previews_like_an_archive`, `a_loose_client_is_read_only_until_a_backup_folder_is_confirmed_then_queues`; US files, GX layout (opt-in): `an_extracted_copy_of_us_client_files_reads_as_a_loose_client` |
| Preview of models, animations, textures (DDS, TGA, BMP), text, hex | yes, for files in the original binary formats; Shaiya-GX says models, animations, world and effect files keep them (docs/data-formats.md lines 30-33) | synthetic tree: `a_loose_client_previews_a_model_with_its_list_texture_in_the_app`, `a_textured_preview_resolves_its_texture_on_a_loose_folder_as_in_an_archive`; US files, GX layout (opt-in): the same file as above |
| Validate: `shaiya info`, `shaiya anim validate`, the Health window | per file, and the Health window for a folder (unreadable files, models that do not read to their last byte, SData wrapper, textures not found) | synthetic tree: `from_a_loose_client_reads_the_file`, `the_health_window_reports_a_loose_folder_and_recovers_an_interrupted_change` |
| Export as JSON | yes (glTF, OBJ and BVH read the files the same way but have no test of their own on a loose folder) | synthetic tree: `export_json_works_from_a_loose_client` |
| Add, replace, remove files | yes, with a confirmed backup folder; every file is checked first and written atomically, the originals go to the journal | synthetic tree: `loose_replace_then_rollback_restores_the_bytes`, `an_added_file_is_deleted_by_rollback`, `a_removed_file_moves_to_the_journal_and_comes_back`, `apply_on_a_loose_client_writes_through_the_loose_journal` |
| Converted media (anything under `interface/`, `Sound/`, `Cursor/`; `.wav`, `.ogg`, `*_dds.png` names; a `.dds` or `NN.tga` whose PNG twin sits beside it) | refused as a whole, nothing written | synthetic tree with the Shaiya-GX sign: `converted_media_is_refused_on_a_gx_layout_and_allowed_on_a_plain_one` |
| Patch apply | yes, for entries whose files are not converted media; one change with its own journal | synthetic tree: `a_patch_applies_to_a_plain_loose_folder_and_rolls_back`, `a_patch_with_interface_entries_is_refused_on_a_gx_layout` |
| Undo | the newest change only, and only while the files are as the change left them | synthetic tree: `only_the_newest_change_rolls_back`, `rollback_is_blocked_when_a_file_changed_since` |
| SData tables on the client's files | the Toolkit's SData tab reads a loose client's `*.SData` files | synthetic tree: `the_sdata_tab_reads_the_files_of_a_loose_client` |

## Server folder (`server/data`)

Shaiya-GX documents the server's INIs and its own `NpcQuest.SData` in `data/game` and the maps in `data/maps` (read from the Shaiya-GX docs: docs/server.md lines 13-19 and 125-150, docs/data-formats.md line 36). That is two folders where the original server keeps them in one `Data` folder with a `Map` child. The synthetic tree used by the tests below is `server/data/game/{Map.ini, Obelisk.ini, NpcQuest.SData}` and `server/data/maps/<id>.svmap`.

| What | Works | Tested on |
|---|---|---|
| `shaiya svmap check MAPS --server-data GAME`, or `shaiya svmap check --server-data GAME --maps MAPS` | the maps and the INIs are read from their two folders and the server and client rules run over them; with `--maps` a missing `MapWar.ini` is an information note, not an error | synthetic tree: `check_with_split_server_folders`, `the_maps_option_needs_server_data_and_excludes_the_target` |
| `shaiya sdata decrypt` or `encrypt` with `--in-place` on a server `NpcQuest.SData` | the file is changed in place, the original is kept as `NpcQuest.SData.bak` and never overwritten, no other file of the folder is touched | synthetic tree: `sdata_in_place_on_a_gx_server_tree_keeps_a_bak` |
| SVMap Editor: `Map.ini` editing with the map folder and the server data folder set separately | a folder you set is the one used, also when the parent of the map folder holds a `Map.ini`; a save keeps a `.bak` once and a timestamped copy in the editor's data folder | synthetic tree: `the_server_data_folder_wins_over_the_svmap_parent`, `map_ini_edit_and_svmap_save_work_on_a_split_gx_tree` |
| SVMap Editor: saving a map of `data/maps` | as for any map folder: checks, `.bak` once, timestamped copy, safe save | synthetic tree: `map_ini_edit_and_svmap_save_work_on_a_split_gx_tree` |
| A `Map.ini` with keys and sections the toolkit does not know | read without an error, every unknown line kept byte for byte by an edit; a key we would write must be letters, digits and `_`; a file without `[BASE] MapCount` is refused by the editor | synthetic file with invented key names: `map_ini_with_extra_keys_is_read_or_refused_as_documented` |
| Other INIs of `data/game` (`ItemBoxes.ini`, `WorldDrops.ini`, `TeleportRunes.ini`, `SafeZones.INI`, ...) | not opened; the toolkit reads only `Map.ini`, `MapWar.ini` and `Obelisk.ini` (zone ids and obelisk positions) of the server's INIs | code: `ServerData::load` in `crates/shaiya-svmap-tools/src/server_data.rs`; `obelisk_ini_lines` |

What the extra keys test says about real Shaiya-GX files: nothing. Shaiya-GX's `Map.ini` is described as holding more than the original keys (docs/gx-studio.md lines 106-110); the test uses invented keys to pin what our parser does with any unknown key. No Shaiya-GX `Map.ini` was read.

## Not supported

- Writing or converting PNG or OGG files, and the game's converted media in general (reading a PNG or OGG header or showing a picture is a separate matter, see [JPG, PNG and OGG](../formats/jpg-png-ogg.md)).
- SQLite: the toolkit has no SQLite code. The Shaiya-GX server keeps its item, skill, NPC-skill and product definitions in an SQLite database (read from docs/server.md and docs/gx-studio.md); Shaiya-GX's own Studio edits them.
- Shaiya-GX's own files and programs: the toolkit never opens, changes or runs `Shaiya GX.exe` or the Studio, and this repository holds none of their game-derived files.
- Converting a Shaiya-GX folder to an original client or back: Shaiya-GX has its own tool for importing an original client.
- Shop editing (NPC shops, the cash shop): not in this version.

## Differences of EP5.4 that we know of

All of this is read from the Shaiya-GX documentation and has not been checked by us against a Shaiya-GX file.

- `Character/Skill.SData` must be the EP5 table with nine slots (docs/data-formats.md lines 34 and 98). The toolkit's `skill-9` layout reads the EP5.4 table of one French EP5.4 client (see [skill tables](../formats/skill-sdata.md)); whether it reads a Shaiya-GX table to its last byte is not known.
- Shaiya-GX says it changed some files: a footer on dungeon files (lines 169-174) and a repaired `Mob_Rend` model (lines 263-294). The toolkit's readers read a file to its last byte and refuse otherwise, so such a file may be refused; this was not tried.
- The format pages carry corrections found by reading the Shaiya-GX documents next to US EP4.5 files, for example [.3DE](../formats/3de.md) and [NpcQuest.SData](../formats/npcquest.md). Those numbers are measured on the US client, not on Shaiya-GX data.

See also the [version matrix](../version-matrix.md#shaiya-gx), which lists the layout row and the install profiles with the same labels.
