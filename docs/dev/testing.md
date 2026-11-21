# Testing

## Policy

**No game data in the repository.** All fixtures are built by `shaiya-testkit` or by the tests themselves; tests that need real files are opt-in and skip with a message. Should a later test need a committed fixture, it goes in `crates/<crate>/tests/fixtures/`, at most 64 KiB, hand-made or generated (never derived from game data), listed with its origin in `crates/<crate>/tests/fixtures/README.md`; `cargo xtask guard` only allows such listed files.

## Levels

1. **Unit tests** in each crate.
2. **Known-answer tests for SEED** (`crates/shaiya-seed/tests/vectors.rs`): produced once by a Python reference implementation and committed as hex, with the exact command in a comment.
3. **Round trips on synthetic data:** an archive built by `SahBuilder` (versions 0 and 1, nested folders, cp949 names, free entries, a trailer) parses and writes back identically; `wrap(unwrap(x)) == x` for plain and encrypted data of many lengths; patch build (twice gives identical bytes), list, verify and apply on a synthetic client; in-place edits and patch applies followed by rollback restore `data.sah` byte for byte and `data.saf` to its old length; an error or cancel during the append leaves both files as they were; every crash point of the pending marker, compaction and swap is simulated and recovered; rollback is blocked after compaction, swap or a newer change; compaction; a cancelled compaction leaves both files byte-identical.
4. **Robustness:** truncation at every byte of a small index, SEED file and patch returns an error; a single-bit flip at every byte never panics; a flip inside a SEED body always fails the checksum.
5. **CLI tests** (`crates/shaiya-cli/tests/{cli,archive,patch,sdata,clients}.rs`) run the real `shaiya` binary (`CARGO_BIN_EXE_shaiya`) with `--data-dir` pointing at a temporary folder, and check text, `--json` keys (parsed with `serde_json`), exit codes, `--dry-run` writing nothing, `@name` resolution and `--help` of every command. A patch end to end: keygen, build, sign, verify, apply with a key, history, rollback, verify against the client.
6. **GUI harness tests** (`shaiya-ui`, `shaiya-toolkit`): headless frames with synthetic input: the empty state, drop, tasks with progress, cancel, the theme switch, the command palette, settings written with `--data-dir` and read back, `--no-settings` writing nothing, the contrast of both palettes; in Shaiya Toolkit also adding a client, opening an archive, search, folder navigation, previews, queueing changes and applying them, the apply and roll-back flow, the patch builder, signing and verifying, the SData tab, and the Retry dialog when a file is open on Windows.
7. **Editor tests** (`shaiya-svmap-editor`): headless harness tests of the app with synthetic input (windows, tools, undo, saving, the Help guide and its lints), plus the opt-in tests below that need `SVMAP_ORIGINALS` or `SHAIYA_CLIENT`. The Windows-only ones are `#[cfg(windows)]`.
8. **Opt-in tests on your own client** (below).
9. **Real-window smoke:** `shaiya-toolkit --smoke report.txt`, and `shaiya-svmap-editor --smoke out.png`. The Toolkit scenes of `--shot` are run headless by `crates/shaiya-toolkit/tests/shots.rs` (each scene leaves the expected tab and data and shows no path); `--shot` itself needs a desktop session ([how the pictures are retaken](docs-site.md#screenshots)).

## The testkit

`shaiya-testkit` (dev-dependency, std only) returns bytes and paths, never toolkit types: `TempDir`, `SahBuilder` (writes `data.sah` / `data.saf` by hand from the documented layout, so it is an oracle and not a mirror of the code under test), `demo_client(dir)` (a synthetic client folder of made-up files, marked by `shaiya-demo-client.txt`: an index with an `Item.SData`, a texture, a box `.smod` and a field `.wld`, `Version.ini`, loose files; `cargo xtask demo-client DIR` writes it for the screenshots), `ZipBuilder` (writes a ZIP by hand with stored entries and entry names exactly as given, so `../x` and case-twin names can be tested without the `zip` crate), `tga_stub` and `bmp_stub` (tiny images for preview tests), `pe_stub`, `truncate`, `flip`, `env_client`, `env_test_clients`, `env_scratch`.

## Opt-in variables

| Variable | Meaning |
|---|---|
| `SHAIYA_CLIENT` | A client folder with `data.sah` and `data.saf` (and `game.exe`; `Version.ini` and a stock pair `data.orig.sah` / `data.orig.saf` are used when present). **Read only.** Tests then check a byte-identical index rewrite, `wrap(unwrap(x)) == x` for every SData, `inspect` on the files, the Monster and NpcQuest readers, the archive library numbers (`check`, `stats`, `diff --quick` against the stock pair when it exists), SData extraction against `Archive::read`, and print timings. |
| `SHAIYA_SCRATCH` | A folder **outside any repository** with room for a few GB, where tests may write. Used together with `SHAIYA_CLIENT`: the tests copy the client's `data.sah`, `data.saf` and `Version.ini` into `SHAIYA_SCRATCH/t1-us-copy-<pid>` and never touch the original. On the copy they build and apply a patch (an SData round trip, an added file, a removal, a loose file) with a journal, check `verify --client` and `check`, roll back and compare SHA-256 with the originals, then run `compact --in-place`, a content `diff` against the original (0 differences) and a swap and swap back, and print MB/s. The copy is removed at the end, also on failure. `shaiya-svmap-tools` (`share::real_client_copy_install_uninstall`) copies `data.sah` and `data.saf` into `SHAIYA_SCRATCH/svmap-share-<pid>`, installs one 28-byte staged file, reads it back, uninstalls, and checks both copies hash as before and the originals' hash, size and modification time are unchanged; it prints the hashes and the `data.saf` growth. |
| `SHAIYA_TEST_CLIENTS` | The matrix folder (below). |
| `SVMAP_ORIGINALS` | A folder with the original server map files (`<id>.svmap`), read only. Used by `shaiya-svmap` and `shaiya-svmap-tools` (with `SHAIYA_CLIENT` for the client rules): byte-identical round trips in binary and JSON, validation totals, folder diff, bulk dry run. Its parent folder is taken as the server data folder (`Map.ini` and friends) for the cross-file checks. |

All of them skip with a message (`skipped: SHAIYA_CLIENT not set`) when unset, so a plain `cargo test --workspace` needs none. Example:

```
SHAIYA_CLIENT=D:\clients\us SHAIYA_SCRATCH=D:\scratch cargo test -p shaiya-matrix-tests --test us_copy -- --nocapture --test-threads=1
```

Client health (`--test health`): with `SHAIYA_CLIENT` and/or `SHAIYA_TEST_CLIENTS` it runs the read-only report on every present client and prints the numbers (`HEALTH <client>: ...`); the folder's entries must be unchanged afterwards. With `SHAIYA_SCRATCH` as well, it copies each client's `data.sah` and an empty `data.saf` into a new folder under the scratch folder, adds three leftovers (`data.sah.bak`, a stray temp file and a swap pair), cleans up, puts back, compares names and bytes, and removes the copy.

## Temp folders on exFAT

Tests write to `TMP` / `TEMP`. When those point at an exFAT drive, replacing a file while another handle on it is still open fails with "Access is denied" on Windows (observed: `write_new_drops_shadowed_entries_and_the_free_list_and_verifies` failed 10 of 10 runs before it closed its reader of the output pair first). A test that replaces a file must drop every handle on it before the replace.
Tests that cancel a background task do not race it: `Tasks::hold_progress` (feature `testing` of `shaiya-ui`) stops every task at its first progress report until the test cancels it, so the cancel always arrives mid-task.

## The version matrix

`crates/shaiya-matrix-tests` holds an `EXPECTED` table (client, capability, `OK | fails | Plain | n/a`) that mirrors [../version-matrix.md](../version-matrix.md), one test per capability that walks the clients, and a test that prints the observed table as Markdown. A cell that differs from `EXPECTED` fails, an improvement included, with the message "update EXPECTED and docs/version-matrix.md". Update both in the same commit as the code.

The T1 rows of the matrix (archive reading, in-place edit and patch apply with rollback on a stub `data.saf`, SData batch work) are listed in [../version-matrix.md](../version-matrix.md); everything that writes does so in a temporary copy.

Folder layout of `SHAIYA_TEST_CLIENTS` (the ids are `ru-ep3`, `ru-ep45`, `ru-ep54`, `fr-ep54`, `phoenix-ep63`, `kr-ep8`, `fawkes-v68`, `cn-ep4`):

```
<root>/<client-id>/data.sah
<root>/<client-id>/game.exe
<root>/<client-id>/files/<archive path with "/" replaced by "__">     (SData files and sysmsg-uni.txt)
```

A client may have a `data.saf` instead of `files/` (then the SData files are read from the archive). The files come from public archives of Shaiya clients that you fetch yourself; this repository does not ship a fetch script, and nothing is ever copied into it.

```
SHAIYA_TEST_CLIENTS=<root> SHAIYA_CLIENT=<us client> cargo test -p shaiya-matrix-tests -- --nocapture --test-threads=1
```

## Performance lines

The matrix tests print: SAH parse time of the largest index, SEED decrypt and encrypt throughput on the client's SData files, and the SHA-256 throughput on `data.saf`. Budgets are in the release documentation ([releasing.md](releasing.md)).
