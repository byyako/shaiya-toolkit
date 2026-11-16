# Testing

## Policy

**No game data in the repository.** All fixtures are built by `shaiya-testkit` or by the tests themselves; tests that need real files are opt-in and skip with a message. Should a later test need a committed fixture, it goes in `crates/<crate>/tests/fixtures/`, at most 64 KiB, hand-made or generated (never derived from game data), listed with its origin in `crates/<crate>/tests/fixtures/README.md`; `cargo xtask guard` only allows such listed files.

## Levels

1. **Unit tests** in each crate.
2. **Known-answer tests for SEED** (`crates/shaiya-seed/tests/vectors.rs`): produced once by a Python reference implementation and committed as hex, with the exact command in a comment.
3. **Round trips on synthetic data:** an archive built by `SahBuilder` (versions 0 and 1, nested folders, cp949 names, free entries, a trailer) parses and writes back identically; `wrap(unwrap(x)) == x` for plain and encrypted data of many lengths; patch build, list and apply on a synthetic client; compaction; a cancelled compaction leaves both files byte-identical.
4. **Robustness:** truncation at every byte of a small index, SEED file and patch returns an error; a single-bit flip at every byte never panics; a flip inside a SEED body always fails the checksum.
5. **CLI tests** (`crates/shaiya-cli/tests/cli.rs`) run the real `shaiya` binary and check text, JSON and exit codes.
6. **GUI harness tests** (`shaiya-ui`, `shaiya-studio`): headless frames with synthetic input: the empty state, drop, tasks with progress, cancel, the theme switch, the command palette, settings written with `--data-dir` and read back, `--no-settings` writing nothing, the contrast of both palettes.
7. **Opt-in tests on your own client** (below).
8. **Real-window smoke:** `shaiya-studio --smoke report.txt`.

## The testkit

`shaiya-testkit` (dev-dependency, std only) returns bytes and paths, never toolkit types: `TempDir`, `SahBuilder` (writes `data.sah` / `data.saf` by hand from the documented layout, so it is an oracle and not a mirror of the code under test), `pe_stub`, `truncate`, `flip`, `env_client`, `env_test_clients`.

## Opt-in variables

| Variable | Meaning |
|---|---|
| `SHAIYA_CLIENT` | A client folder with `data.sah` and `data.saf` (and `game.exe`). Tests then check a byte-identical index rewrite, `wrap(unwrap(x)) == x` for every SData, `inspect` on the files, the Monster and NpcQuest readers, and print timings. |
| `SHAIYA_TEST_CLIENTS` | The matrix folder (below). |

## The version matrix

`crates/shaiya-matrix-tests` holds an `EXPECTED` table (client, capability, `OK | fails | Plain | n/a`) that mirrors [../version-matrix.md](../version-matrix.md), one test per capability that walks the clients, and a test that prints the observed table as Markdown. A cell that differs from `EXPECTED` fails, an improvement included, with the message "update EXPECTED and docs/version-matrix.md". Update both in the same commit as the code.

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
