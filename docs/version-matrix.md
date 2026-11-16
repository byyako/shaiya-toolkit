# Version matrix

What the toolkit parses on which client. This table mirrors `EXPECTED` in `crates/shaiya-matrix-tests/src/lib.rs`; the matrix tests fail when a result differs, an improvement included, and the code, the table in the test and this page are updated in the same commit.

Client ids: `us-108` (US patch level 108), and the eight test clients `ru-ep3`, `ru-ep45`, `ru-ep54`, `fr-ep54`, `phoenix-ep63`, `kr-ep8`, `fawkes-v68`, `cn-ep4` (region and episode). `ru-ep54` has an index only: its data file is not available.

| Capability | us-108 | ru-ep3 | ru-ep45 | ru-ep54 | fr-ep54 | phoenix-ep63 | kr-ep8 | fawkes-v68 | cn-ep4 |
|---|---|---|---|---|---|---|---|---|---|
| index files | 19,939 | 21,523 | 23,533 | | 24,599 | 30,340 | 34,419 | 48,119 (v1) | 20,418 |
| `Index::parse` | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `to_bytes` byte-identical | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `inspect` on data.sah | OK | OK | OK | OK | OK | OK | OK | OK | OK |
| `seed::unwrap` + CRC, every SData | OK | OK | OK | n/a | OK | OK | OK | OK | Plain |
| `wrap(unwrap(x)) == x`, every SData | OK | OK | OK | n/a | OK | OK | OK | OK | OK (plain) |
| clientdata `Monsters::parse` | OK | OK | OK | n/a | OK | OK | fails | fails | fails |
| clientdata `NpcQuest::parse` | OK | OK | OK | n/a | OK | OK | fails | fails | OK |
| `inspect` on game.exe (PE) | OK | OK | OK | n/a | OK | OK | OK | OK | OK |

Reading the table:

- **fails** asserts that parsing returns an error (the right failure mode for a layout this version does not know); a future schema fix shows up as a test asking for the table to be updated.
- **Plain**: every SData of that client is stored without the SEED wrapper.
- **n/a**: the file is not part of the test client.
- The file counts other than the US client's come from an earlier survey of the same clients; the matrix run prints the observed counts, and a difference is reported rather than adjusted.
- The `fawkes-v68` index is version 1.
- The `Monsters` and `NpcQuest` rows are the layouts of the original client generation; later episodes change them. Schema support for those is planned work.

Run it on your own copies: [dev/testing.md](dev/testing.md#the-version-matrix).
