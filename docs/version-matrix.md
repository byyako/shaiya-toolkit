# Version matrix

What the toolkit parses on which client. This table mirrors `EXPECTED` and `NUMBERS` in `crates/shaiya-matrix-tests/src/lib.rs`; the matrix tests fail when a result differs, an improvement included, and the code, the table in the test and this page are updated in the same commit.

Client ids: `us-108` (US patch level 108), and the eight test clients `ru-ep3`, `ru-ep45`, `ru-ep54`, `fr-ep54`, `phoenix-ep63`, `kr-ep8`, `fawkes-v68`, `cn-ep4` (region and episode). `ru-ep54` has an index only: its data file is not available, so every capability that needs file contents is `n/a` there.

| Capability | us-108 | ru-ep3 | ru-ep45 | ru-ep54 | fr-ep54 | phoenix-ep63 | kr-ep8 | fawkes-v68 | cn-ep4 |
|---|---|---|---|---|---|---|---|---|---|
| index entries | 19,939 | 21,523 | 23,533 | 24,745 | 24,599 | 30,340 | 34,419 | 48,119 (v1) | 20,418 |
| `Index::parse` | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `to_bytes` byte-identical | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `inspect` on data.sah | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `seed::unwrap` + CRC, every SData | OK | OK | OK | n/a | OK | OK | OK | OK | Plain |
| `wrap(unwrap(x)) == x`, every SData | OK | OK | OK | n/a | OK | OK | OK | OK | OK (plain) |
| clientdata `Monsters::parse` | OK | OK | OK | n/a | OK | OK | fails | fails | fails |
| clientdata `NpcQuest::parse` | OK | OK | OK | n/a | OK | OK | fails | fails | OK |
| `inspect` on game.exe (PE) | OK | OK | OK | n/a | OK | OK | OK | OK | OK |
| `archive_visible_count` (files minus shadowed) | OK 19,939 | OK 21,523 | OK 23,533 | OK 24,745 | OK 24,599 | OK 30,340 | OK 34,418 | OK 48,118 | OK 20,418 |
| `archive_check_index` (index-only check: 0 problems; the number is the count of `shadowed` warnings) | OK 0 | OK 0 | OK 0 | OK 0 | OK 0 | OK 0 | OK 1 | OK 1 | OK 0 |
| `archive_free_list` (free-list entries preserved by an in-place edit) | OK 0 | OK 0 | OK 507 | OK 500 | OK 500 | OK 0 | OK 0 | OK 630 | OK 500 |
| `archive_data_end` (`max(offset + length)`, 64-bit; past 4 GiB on fawkes) | OK | OK | OK | OK | OK | OK | OK | OK 6,645,531,195 | OK |
| `archive_diff_self` (`diff --quick` of the index with itself: all same) | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `archive_edit_rollback_stub` (in-place add of 3 files including a code page 949 name, replace 1, remove 1 on a copy of the real index with an empty `data.saf`; rollback restores `data.sah` byte for byte and `data.saf` to length 0) | OK | OK | OK | OK | OK | OK | OK | OK (v1) | OK |
| `patch_apply_rollback_stub` (the same through a synthetic patch with a loose file) | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `sdata_batch_check` (`sdata check` over every SData file) | OK | OK | OK | n/a | OK | OK | OK | OK | Plain |
| `sdata_like_roundtrip` (decrypt, then `encrypt --like` the originals: byte-identical) | OK | OK | OK | n/a | OK | OK | OK | OK | OK (plain stays plain) |
| clientdata `Wld::parse`, every `world/*.wld` | OK 83 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `wld_raw` writes every `.wld` back byte-identical | OK 83 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| map preview data of every `world/*.wld` (`render::hillshade_grey` for a field, no terrain for a dungeon) | OK 83 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| model preview data of every `.smod` (`render::model_picture`: modules, triangles, textures looked up under `Entity/Texture`) | OK 1,355 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `area_txt` writes every `world/*.txt` back byte-identical | OK 33 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| gatekeepers: 3 gates each, map ids 0..=255, finite positions (the number is the count of gatekeepers) | OK 61 | OK 61 | OK 72 | n/a | OK 72 | OK 106 | fails | fails | OK 61 |
| sysmsg has text for the distinct map-name message ids of the EP4.5 table (the number is the message ids with text, of 47) | OK 47 | OK 47 | OK 47 | n/a | OK 47 | OK 47 | OK 47 | OK 47 | OK 44 |
| sdata `Item` layout fits (the number is the records read from `Item/Item.SData`) | OK 16,045 | OK 15,612 | OK 16,559 | n/a | OK 17,090 | OK 22,697 | FIT-ONLY 25,245 | FIT-ONLY 24,990 | FIT-ONLY 16,047 |
| sdata `Skill` layout fits (the number is the records read from `Character/Skill.SData`: entries x levels) | OK 978 | OK 882 | OK 978 | n/a | OK 3,348 | OK 6,765 | FIT-ONLY 8,775 | FIT-ONLY 8,460 | FIT-ONLY 978 |
| sdata `Monster` layout fits (the number is the records read from `Monster/Monster.SData`) | OK 2,474 | OK 2,372 | OK 2,474 | n/a | OK 2,992 | OK 4,511 | FIT-ONLY 4,776 | FIT-ONLY 4,774 | fails |
| sdata `NpcQuest` layout fits (the number is the quests read from `Npc/NpcQuest.SData`) | OK 1,720 | OK 1,531 | OK 2,109 | n/a | OK 2,262 | OK 3,051 | FIT-ONLY 3,820 | FIT-ONLY 4,182 | FIT-ONLY 1,531 |
| `auto` picks exactly one layout or none, every SData (the number is the files that exactly one layout reads; the rest are shown raw) | OK 5 | OK 5 | OK 5 | n/a | OK 5 | OK 5 | OK 6 | OK 5 | OK 6 |
| `.ANI` files the index lists (index only: the number is the visible `.ANI` files of the index; no contents were read on the eight test clients) | OK 3,509 | OK 3,537 | OK 3,624 | OK 3,724 | OK 3,723 | OK 4,099 | OK 4,255 | OK 5,951 | OK 3,429 |
| `Ani::parse` reads every `.ANI` to the last byte and `to_bytes` gives it back byte for byte; `validate` finds no error | OK 3,509 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `modeljson::export` then `import` gives every `.ANI` back byte for byte | OK 3,509 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.3DC` files the index lists (index only: the number is the visible `.3DC` files of the index; no contents were read on the eight test clients) | OK 2,431 | OK 2,635 | OK 3,049 | OK 3,164 | OK 3,126 | OK 4,988 | OK 4,220 | OK 6,534 | OK 2,317 |
| `C3dc::parse` reads every `.3DC` to the last byte with exactly one layout and `to_bytes` gives it back byte for byte | OK 2,431 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `modeljson::export` then `import` gives every `.3DC` back byte for byte | OK 2,431 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Animation preview: `.ANI` files that have at least one compatible `.3DC` mesh (found by name and bone count; validated offline) | OK 3,365 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.VANI` files the index lists (index only: the number is the visible `.VANI` files of the index; no contents were read on the eight test clients) | OK 66 | OK 103 | OK 116 | OK 116 | OK 116 | OK 129 | OK 129 | OK 141 | OK 74 |
| `Vani::parse` reads every `.VANI` to the last byte and `to_bytes` gives it back byte for byte | OK 66 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `modeljson::export` then `import` gives every `.VANI` back byte for byte | OK 66 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.MANI` files the index lists (index only: the number is the visible `.MANI` files of the index; no contents were read on the eight test clients) | OK 23 | OK 39 | OK 39 | OK 39 | OK 39 | OK 54 | OK 48 | OK 64 | OK 35 |
| every `.MANI` the index lists is 108 bytes (index only: the length each index gives; the number is the files of that length) | OK 23 | OK 39 | OK 39 | OK 39 | OK 39 | OK 54 | OK 48 | OK 64 | OK 35 |
| `Mani::parse` reads every `.MANI` (108 bytes) and `to_bytes` gives it back byte for byte | OK 23 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `modeljson::export` then `import` gives every `.MANI` back byte for byte | OK 23 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.3DO` files the index lists (index only: the number is the visible `.3DO` files of the index; no contents were read on the eight test clients) | OK 271 | OK 300 | OK 414 | OK 415 | OK 415 | OK 461 | OK 442 | OK 514 | OK 298 |
| `Mesh3do::parse` reads every `.3DO` to the last byte and `to_bytes` gives it back byte for byte | OK 271 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `modeljson::export` then `import` gives every `.3DO` back byte for byte | OK 271 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.3DE` files the index lists (index only: the number is the visible `.3DE` files of the index; no contents were read on the eight test clients) | OK 469 | OK 640 | OK 718 | OK 756 | OK 708 | OK 892 | OK 925 | OK 1,344 | OK 611 |
| `Mesh3de::parse` reads a `.3DE` with exactly one of its two layouts and `to_bytes` gives it back byte for byte; any other file is refused with the reason (the number is the files that read) | OK 467 of 469 (2 refused, named on the format page) | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `modeljson::export` then `import` gives every `.3DE` the reader reads back byte for byte; the files it refuses stay refused (the number is the files that came back) | OK 467 of 469 (2 refused, named on the format page) | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.MLT` files the index lists (index only: the number is the visible `.MLT` files of the index; no contents were read on the eight test clients) | OK 112 | OK 112 | OK 112 | OK 112 | OK 112 | OK 125 | OK 133 | OK 134 | OK 112 |
| `Mlt::parse` reads every `.MLT` to the last byte and `to_bytes` gives it back byte for byte | OK 112 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.MON` files the index lists (index only: the number is the visible `.MON` files of the index; no contents were read on the eight test clients) | OK 16 | OK 16 | OK 16 | OK 16 | OK 16 | OK 16 | OK 16 | OK 17 | OK 16 |
| `Mon::parse` reads every `.MON` to the last byte and `to_bytes` gives it back byte for byte | OK 16 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.ITM` files the index lists (index only: the number is the visible `.ITM` files of the index; no contents were read on the eight test clients) | OK 17 | OK 17 | OK 17 | OK 17 | OK 17 | OK 17 | OK 17 | OK 17 | OK 17 |
| `Itm::parse` reads every `.ITM` to the last byte and `to_bytes` gives it back byte for byte | OK 17 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Texture index: `.3DC` and `.3DO` meshes that the `.MLT`, `.MON` and `.ITM` lists pair with at least one texture that exists (the number is the meshes) | OK 2,237 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `.wav` files the index lists (index only: the number is the visible `.wav` files of the index; no contents were read on the eight test clients) | OK 1,019 | OK 1,024 | OK 1,024 | OK 1,076 | OK 1,679 | OK 1,207 | OK 1,153 | OK 1,257 | OK 1,587 |
| `wav::parse_header` reads the header of every `.wav` (the number is the files) | OK 1,019 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| `texture::decode` decodes every `.jpg` to RGBA (the number is the files) | OK 22 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |

Reading the table:

- **fails** asserts that parsing returns an error (the right failure mode for a layout this version does not know); a future schema fix shows up as a test asking for the table to be updated.
- **Plain**: every SData of that client is stored without the SEED wrapper.
- **n/a**: the file is not part of the test client.
- **OK 19,939** and similar: the test asserts that exact number. Other counts (for example zero-length files) are printed by the matrix run, not asserted.
- Index entries counts every file entry; visible count removes the shadowed ones (one on kr-ep8 and on fawkes-v68, which is the `archive_check_index` number).
- The `fawkes-v68` index is version 1.
- The sdata rows measure the table viewer (`shaiya-sdata`). A layout fits a file only when it reads the whole plaintext to the last byte with plausible counts. **OK** is a fit by a layout that was read from that client's own table loader (the layout file's `evidence.loader` lists the client); **FIT-ONLY** is a fit by a layout that was not read from that client's loader (`evidence.fit_only` lists it): the data is read to the last byte, nothing more, and the number is the records read. **fails** is "no layout reads this file" (the viewer shows it as hex). None of the three is a verdict on the file. A FIT-ONLY cell is never called OK: the packed `game.exe` of kr-ep8, cn-ep4 and fawkes-v68 gives no loader to read. The layouts are one per format variant: `Item` 81, 132 or 134 bytes per record, `Skill` with 3, 9 or 15 levels per entry, `Monster` with a 31 or 36 byte block, `NpcQuest` with a 165, 87 or 287 byte quest block (see [item-sdata.md](formats/item-sdata.md), [skill-sdata.md](formats/skill-sdata.md), [monster.md](formats/monster.md), [npcquest.md](formats/npcquest.md)). Which variant read which client, and on which clients it is loader-verified (L) or FIT-ONLY (F):
  - `item-81`: L us-108, ru-ep3, ru-ep45, fr-ep54; F cn-ep4. `item-132`: L phoenix-ep63. `item-134`: F kr-ep8, fawkes-v68 (a FIT-ONLY layout: no loader read, positional column names).
  - `skill-3`: L us-108, ru-ep3, ru-ep45; F cn-ep4. `skill-9`: L fr-ep54; F kr-ep8 (its `Npc/NpcSkill.SData`). `skill-15`: L phoenix-ep63; F kr-ep8, fawkes-v68.
  - `monster-31`: L us-108, ru-ep3, ru-ep45, fr-ep54, phoenix-ep63. `monster-36`: F kr-ep8, fawkes-v68 (FIT-ONLY). cn-ep4 Monster: **no fit** (neither `monster-31` nor `monster-36` fits it), so the cell says `fails` and the file is shown as hex.
  - `npcquest-165`: L us-108, ru-ep3, ru-ep45, fr-ep54; F cn-ep4. `npcquest-87`: L phoenix-ep63. `npcquest-287`: F kr-ep8, fawkes-v68 (FIT-ONLY).
  - The layout files carry this as their `evidence` object, and the matrix tests fail when the layout that `auto` picks for a client's file does not list that client. ru-ep54 has no SData in the test set, so nothing is fitted there. The `auto` row counts, per client, the SData files that exactly one layout reads (`Item`, `Skill`, `NpcSkill` as a skill table, `Monster`, `NpcQuest`); every other SData (other tables, the BinarySData files of kr-ep8 and fawkes-v68) stays raw. No client has a file that two layouts read (that would be shown raw too). A FIT-ONLY layout becomes loader-verified only when it is read from that client's own loader and still reads every file of that table to the last byte; see [FIT-ONLY layouts](formats/item-sdata.md#fit-only-layouts).
- The `Monsters` and `NpcQuest` rows are the layouts of the original client generation; later episodes change them. Schema support for those is planned work.
- The two `_stub` rows edit a copy of each client's real index against an empty `data.saf`, so no client file is changed and no client contents are needed. Archive operations that need a full `data.saf` (check with the data file, extract, compaction, swap) are covered by synthetic tests and, for one client you supply yourself, by the opt-in tests in [dev/testing.md](dev/testing.md#opt-in-variables).
- World files are not part of the eight test clients (they hold SData files and `sysmsg-uni.txt` only), so the `.wld` and `world/*.txt` rows are `n/a` there; only the US client's full `data.saf` measures them.
- The map preview row counts the `.wld` files whose preview data is built: a field gives a grey picture of its terrain, a dungeon map has no terrain and no picture. The split of the 83 files is printed by the matrix run and the toolkit's opt-in test.
- The model preview row counts the `.smod` files whose preview data is built: all 1,355 of the US client give a picture, with 0 texture names missing; the toolkit's opt-in test asserts both and the matrix run prints the counts. The 29 fields and 54 dungeons of the map row are asserted too.
- The gatekeeper row is `fails` where `NpcQuest::parse` fails (kr-ep8, fawkes-v68), and `n/a` where there is no file (ru-ep54). The gates were not measured on those two clients: `NpcQuest::parse` fails first there (the layout differs from the original generation's), so the gate row says `fails`.
- The map-name row measures the presence of text for the distinct map-name message ids of the EP4.5 table (47 message ids covering 67 map ids), not its correctness; the cn-ep4 sysmsg lacks text for three of them.
- The `.ANI` rows: the first counts the `.ANI` files each client's index lists; it reads no file contents. The second reads every `.ANI` on `us-108` (US EP4.5) and is `n/a` on the eight test clients because the test set holds no `data.saf` for them: their animations are counted from the index only and were never read. Whether a later episode's `.ANI` files read is not checked. See [ani.md](formats/ani.md).
- The `.3DC` rows work the same way: the first counts the `.3DC` files each client's index lists (no contents read); the second reads every `.3DC` on `us-108` (US EP4.5) with exactly one of the three layouts and is `n/a` on the eight test clients (no `data.saf` in the test set; index only). Whether a later episode's `.3DC` files read is not checked. See [3dc.md](formats/3dc.md).
- The animation preview row counts the `.ANI` files of `us-108` (US EP4.5) of the 3,509 that have at least one `.3DC` mesh in the sibling folder whose name fits and whose vertices name only bones the animation has; the rest show the skeleton only. The check is validated offline (the picture is an approximation of the game's playback and was not compared with the game). It is `n/a` on the eight test clients: their indexes list `.ANI` and `.3DC` files but no contents were read (index only). See [ani.md](formats/ani.md).
- The `.VANI` rows work the same way as the `.ANI` rows: the first counts the `.VANI` files each client's index lists (no contents read); the second reads every `.VANI` on `us-108` (US EP4.5) to the last byte and is `n/a` on the eight test clients (no `data.saf` in the test set; index only). Whether a later episode's `.VANI` files read is not checked. See [vani.md](formats/vani.md).
- The `.MANI` rows: the first counts the `.MANI` files each client's index lists; the second says that every one of them has a length of 108 bytes in the index (the length is read from the index, never from file contents, so it holds for all nine clients); the third reads every `.MANI` on `us-108` (US EP4.5) and is `n/a` on the eight test clients (index only). The meanings of the fields are inferred and were not checked in the game. See [mani.md](formats/mani.md).
- The `.3DO` rows work the same way as the `.ANI` rows: the first counts the `.3DO` files each client's index lists (no contents read); the second reads every `.3DO` on `us-108` (US EP4.5) to the last byte and is `n/a` on the eight test clients (no `data.saf` in the test set; index only). Only one file of `us-108` has frames. Whether a later episode's `.3DO` files read is not checked. See [3do.md](formats/3do.md).
- The `.3DE` rows: the first counts the `.3DE` files each client's index lists (no contents read). The second reads every `.3DE` on `us-108` (US EP4.5) with exactly one of its two layouts: 467 of the 469 files read and write back byte for byte, and the other 2 (`004.3de` and `Deathknight007.3DE`) follow neither layout and are refused with the reason, never half-read; the cell is `OK` because nothing else failed. It is `n/a` on the eight test clients (index only). Whether a later episode's `.3DE` files read is not checked. See [3de.md](formats/3de.md).
- The JSON rows (`.ANI`, `.3DC`, `.VANI`, `.MANI`, `.3DO`, `.3DE`) read every file of that kind on `us-108` (US EP4.5), export it with `modeljson::export` and import the text with `modeljson::import`; the cell is `OK` when every file comes back byte for byte (validated offline). They are `n/a` on the eight test clients (no `data.saf` in the test set; index only). For `.3DE` the 2 files the reader refuses must be refused by the export too. See the format pages.
- The list-file rows (`.MLT`, `.MON`, `.ITM`) work the same way as the `.3DO` rows: the first of each pair counts the files each client's index lists (no contents read); the second reads every file of that kind on `us-108` (US EP4.5) to the last byte and writes it back byte for byte (112 `.MLT`, 16 `.MON` and 17 `.ITM`; validated offline). They are `n/a` on the eight test clients (index only). See [mlt.md](formats/mlt.md), [mon.md](formats/mon.md) and [itm.md](formats/itm.md).
- The texture-index row reads every list file of `us-108` (US EP4.5) and counts the meshes that have a pair whose texture file exists in the archive (validated offline); it is `n/a` on the eight test clients (index only). See [textures.md](formats/textures.md).

## SVMap Editor and `shaiya svmap`

Shaiya SVMap Editor and `shaiya svmap` were written for and verified on the US EP4.5 client (patch 108), the original EP4.5 server files and the Afterglow server. There is no detection of the client version. Every file is checked by whether it reads: a map file that does not read is refused, and a client table that does not read turns off the checks that need it, with a note. The table says what was verified and what happens on other clients. The assumptions:

| Assumption | Verified on | On anything else |
|---|---|---|
| `.svmap` layout ([svmap.md](formats/svmap.md)) | the 77 original EP4.5 server maps, Afterglow | The reader must end at the last byte. A file that does not parse is refused (editor: an error, `shaiya svmap`: exit 3). EP3 and EP5.4 maps are reported to share the layout: unverified. |
| `.wld` layout ([wld.md](formats/wld.md)) | US EP4.5, 83 of 83 byte-identical | Checked per file. A file that does not write back stays read-only on the client side, and `shaiya svmap wld-check` names it (exit 1 or 3). |
| `world/<id>.txt` ([area-txt.md](formats/area-txt.md)) | US EP4.5 | A line-preserving model: lines it does not understand are kept. |
| Name code page cp949 | US EP4.5 | A parameter in the editor's Preferences: cp949, Shift-JIS, GBK or Windows-1252. |
| `Monster.SData` and `NpcQuest.SData` layouts, with the gatekeeper gates ([monster.md](formats/monster.md), [npcquest.md](formats/npcquest.md)) | US EP4.5. Both parse on ru-ep3, ru-ep45, fr-ep54 and phoenix-ep63, and NpcQuest on cn-ep4. | A parse failure turns off the rules that need the table, with one note. `Monster.SData` fails on kr-ep8, fawkes-v68 and cn-ep4, and `NpcQuest.SData` on kr-ep8 and fawkes-v68 (the matrix rows above). |
| The table of map names, mirrored from the EP4.5 client ([sysmsg.md](formats/sysmsg.md)) | US EP4.5 patch 108 | Used for display and for the info rule `map-no-name`, never to decide a write. On another client the names and that rule may be wrong, and a note says so. Without a client the map number is shown. |
| Map id limits: `MapCount` for the original server, 0 to 255 except 200 for Afterglow | the original server, Afterglow | The Target server setting. Other server builds are unverified. |
| The client forces size 1600 on map 70, and ids 100 to 200 limit item use | US EP4.5 | The wizard warnings say "on the EP4.5 client". |
| Field chunk textures: `_L.dds` 1024 DXT1 and `_A<k>.dds` 256 DXT5, the headers of the original files ([textures.md](formats/textures.md)) | US EP4.5 (map 110 loads) | The wizard note says the files follow the EP4.5 layout and other episodes are unverified. |
| Loose files lose to the archive | US EP4.5 | Copy loose refuses every path the archive holds, which is safe for either order. |
| `data.sah` with a header | all nine matrix clients | A `data.sah` without a header is refused by Install. Use an extracted folder to read it. |
| Minimap `interface/MiniMap/<id>.tga`, 512 x 512 | US EP4.5 | Written only when ticked. |
| SEED wrapper checks of `.SData` files: the checksum, a second wrapper inside the first, plain against encrypted ([sdata-wrapper.md](formats/sdata-wrapper.md)) | all eight clients that have SData files: 0 files reported as encrypted twice on every one, which the opt-in test asserts; the other codes are printed per client, not asserted (8 files on us-108, 9 on ru-ep3, 9 on ru-ep45, 9 on fr-ep54, 11 on phoenix-ep63, 14 on kr-ep8, 17 on fawkes-v68, 9 on cn-ep4; ru-ep54 has none). `sdata-state-differs` needs the client's copy and was not measured. | Findings only warn; the Toolkit asks once for a double wrapper or a bad checksum. |
| Texture check: a `.dds` or `.tga` is fine when the toolkit's own reader opens it ([textures.md](formats/textures.md)) | US EP4.5 only (the other clients' archives are not in the test set): 9,819 textures, 0 refused (`Item/DDS/reflection.dds` is a DXT2 DDS; the reader reads DXT1 to DXT5) | The finding says the game may still accept the file; it never blocks. |

When a client opens, the editor (on the client chip) and `shaiya svmap check`, `spawns` and `bulk` (as `note:` lines and in the JSON `notes`) say when `Monster.SData` or `NpcQuest.SData` is missing or did not read with the EP4.5 layout, and how many ids of the map-name table the client's `sysmsg-uni.txt` lacks. What was checked in the game and what was not is in [the editor guide](tools/svmap-editor.md#what-was-checked-in-the-game).

Run it on your own copies: [dev/testing.md](dev/testing.md#the-version-matrix).

## Shaiya-GX

Shaiya-GX (EP5.4) is not a column of the table above: no Shaiya-GX file contents were opened, so no count is asserted. The row and the table below say what is known and from where; the page [Works with Shaiya-GX](guides/shaiya-gx.md) has the same labels in full.

| Capability | Shaiya-GX (EP5.4): layout reference, no file contents |
|---|---|
| index entries, `Index::parse`, `to_bytes` | n/a (no `data.sah`; the client data is a loose folder, read from Shaiya-GX docs/data-formats.md lines 3-16) |
| file formats of models, animations, world, effects, SData | original binary formats, read from Shaiya-GX docs/data-formats.md lines 30-33; not checked by us on a Shaiya-GX file |
| images, icons, audio | PNG and Ogg Vorbis, read from Shaiya-GX docs/data-formats.md lines 22-36 and 79-89; the toolkit writes neither |
| `Character/Skill.SData` | EP5, nine slots, read from Shaiya-GX docs/data-formats.md lines 34 and 98; our `skill-9` layout reads one EP5.4 client's table (the `fr-ep54` column), not a Shaiya-GX table |

| Install profile | What the tests used | Tests |
|---|---|---|
| Client: loose data folder (`Client/data`) | a synthetic folder; US EP4.5 files copied into a Shaiya-GX-shaped folder (opt-in) | `a_loose_client_browses_and_previews_like_an_archive`, `loose_replace_then_rollback_restores_the_bytes`, `converted_media_is_refused_on_a_gx_layout_and_allowed_on_a_plain_one`; opt-in `an_extracted_copy_of_us_client_files_reads_as_a_loose_client` |
| Server: split folders (`data/game` for the INIs and `NpcQuest.SData`, `data/maps` for the maps) | a synthetic tree | `check_with_split_server_folders`, `sdata_in_place_on_a_gx_server_tree_keeps_a_bak`, `map_ini_edit_and_svmap_save_work_on_a_split_gx_tree` |
