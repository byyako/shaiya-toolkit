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
| `area_txt` writes every `world/*.txt` back byte-identical | OK 33 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| gatekeepers: 3 gates each, map ids 0..=255, finite positions (the number is the count of gatekeepers) | OK 61 | OK 61 | OK 72 | n/a | OK 72 | OK 106 | fails | fails | OK 61 |
| sysmsg has text for the ids of the EP4.5 map-name table (the number is the ids with text, of 47) | OK 47 | OK 47 | OK 47 | n/a | OK 47 | OK 47 | OK 47 | OK 47 | OK 44 |

Reading the table:

- **fails** asserts that parsing returns an error (the right failure mode for a layout this version does not know); a future schema fix shows up as a test asking for the table to be updated.
- **Plain**: every SData of that client is stored without the SEED wrapper.
- **n/a**: the file is not part of the test client.
- **OK 19,939** and similar: the test asserts that exact number. Other counts (for example zero-length files) are printed by the matrix run, not asserted.
- Index entries counts every file entry; visible count removes the shadowed ones (one on kr-ep8 and on fawkes-v68, which is the `archive_check_index` number).
- The `fawkes-v68` index is version 1.
- The `Monsters` and `NpcQuest` rows are the layouts of the original client generation; later episodes change them. Schema support for those is planned work.
- The two `_stub` rows edit a copy of each client's real index against an empty `data.saf`, so no client file is changed and no client contents are needed. Archive operations that need a full `data.saf` (check with the data file, extract, compaction, swap) are covered by synthetic tests and, for one client you supply yourself, by the opt-in tests in [dev/testing.md](dev/testing.md#opt-in-variables).
- World files are not part of the eight test clients (they hold SData files and `sysmsg-uni.txt` only), so the `.wld` and `world/*.txt` rows are `n/a` there; only the US client's full `data.saf` measures them.
- The gatekeeper row is `fails` where `NpcQuest::parse` fails (kr-ep8, fawkes-v68), and `n/a` where there is no file (ru-ep54).
- The map-name row measures the presence of text for the ids of the EP4.5 map-name table, not its correctness. The table has 47 ids; the cn-ep4 sysmsg lacks text for three of them.

Run it on your own copies: [dev/testing.md](dev/testing.md#the-version-matrix).
