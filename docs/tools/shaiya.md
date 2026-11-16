# The `shaiya` command line

```
shaiya [GLOBAL FLAGS] <COMMAND> [ARGS]
```

Commands in this version: `info` and `version` (and clap's `help`). The command groups `archive`, `patch`, `sdata`, `log` and later `svmap` are reserved for later releases.

## Global flags

They may come before or after the command.

| Flag | Meaning |
|---|---|
| `--json` | Print **one JSON document** on stdout instead of text. |
| `-q`, `--quiet` | No progress and no notes; errors still go to stderr. |
| `-v`, `--verbose` | More detail (elapsed times); repeatable. |
| `--progress <auto\|always\|never>` | `auto` shows progress only when stderr is a terminal; `always` prints a line per 10% (useful for logs). |
| `-h`, `--help` | Help. |
| `-V`, `--version` | The version line. |

## Conventions (every command follows them)

- Writing commands take `-o, --out PATH`. `--in-place` writes the input after making a backup (`<file>.bak`, or `<file>.<UTC stamp>.bak` when that exists). `--force` allows overwriting an existing output that is not the input. `--dry-run` prints what would be written and writes nothing. A command that would change a file but was given neither `--out` nor `--in-place` stops with exit code 4. (No command in this version writes; the rule is fixed for the next ones.)
- Archive paths are shown with `/`, matched case-insensitively, and taken as UTF-8 then encoded in code page 949 for the lookup.
- Human output goes to **stdout**; progress, notes and errors go to **stderr** (`error: <message>`, optional `hint: <text>` lines).
- `--json`: stdout holds exactly one JSON value, the result object or an error object, for example `{"error": {"kind": "format", "message": "Item.SData: SEED checksum failed (at byte 40)", "file": "Item.SData", "offset": 40}}`. Keys are snake_case, sizes are integers in bytes, hashes lower-case hex, times ISO-8601 UTC, archive paths use `/`. Adding a key is not a breaking change; renaming or removing one is (CHANGELOG "Breaking").
- Progress line (stderr, at most 10 updates per second): `[stage]  45%  900.0 MB / 2.0 GB  120.0 MB/s  ETA 8 s`.
- Writes are atomic (temporary file, then rename), so an interrupted run leaves the input as it was and at most a `*.shaiya-tmp` file, which a later run removes after 24 hours.
- On Windows, when `shaiya.exe` is started by double-click, it prints the help and "This is a command-line program. Open a terminal and run `shaiya --help`." and waits for Enter.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success; checks passed; no differences. |
| 1 | The answer is "no": differences found, a check or verification failed. |
| 2 | Usage error (unknown flag, missing argument). |
| 3 | Input error: unreadable or missing file, bad format, checksum failure, unsupported version. |
| 4 | Refused by a safety rule. |
| 5 | Internal error (a panic); the message asks for a bug report. |
| 130 | Cancelled. |

A command over several inputs continues past bad inputs, reports each, and exits with the highest code met.

## `shaiya info [--hash] <PATH>...`

Identifies each file **by content first, name second** and describes it:

```
data.sah  SAH index, version 0, 19,939 files in 166 folders, 0 free entries, data ends at 1,991,954,802
          771,851 bytes
Item.SData  SData, SEED-encrypted, plaintext 4,512,768 bytes, checksum OK
game.exe  Windows program (x86), linked 2010-04-26 03:12:44 UTC, 3,203,072 bytes
```

Kinds: SAH index, SAF data (with the file count and live bytes from a `.sah` next to it), SData (SEED-encrypted, checksum checked; or plain), patch (archive files, loose files, removals, whether a `.sig` sits next to it), patch signature, Windows program (machine and link time), unknown (the first 16 bytes). A damaged file is reported with a `problem:` line, not a failure; files over 512 MiB are not decrypted for the checksum check.

`--hash` adds a `sha256 <64 hex digits>` line per file and runs with progress (files over 64 MiB show the progress line).

Exit code 0 when every path was read and none has problems; 3 when a path could not be read or has problems. With `--json` the result is `{"files": [ ... ]}` with one object per path (`path`, `size`, `kind`, `details`, `sha256`, `problems`); paths that could not be read at all are listed under `errors`.

## `shaiya version`

```
shaiya 0.1.0 (git 0123456789ab, x86_64-pc-windows-msvc)
```

With `--json`: `{"version": "0.1.0", "git": "0123456789ab", "target": "x86_64-pc-windows-msvc"}`. `shaiya -V` prints the same line.
