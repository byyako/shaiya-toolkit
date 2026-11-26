# The `shaiya` command line

```
shaiya [GLOBAL FLAGS] <COMMAND> [ARGS]
```

| Command | What it does |
|---|---|
| [`info`](#shaiya-info) | Identify files and describe them |
| [`archive`](#shaiya-archive) | List, check, extract, edit, compare, pack, swap `data.sah` / `data.saf` |
| [`patch`](#shaiya-patch) | Build, verify, apply and roll back `psNNNN.patch` files; sign them |
| [`sdata`](#shaiya-sdata) | Describe, check, decrypt and encrypt SData files; show a table as CSV or JSON |
| [`anim`](#shaiya-anim) | Describe `.ANI`, `.3DC`, `.VANI`, `.MANI`, `.3DO` and `.3DE` files and check `.ANI` files; compare a candidate `.ANI`'s skeleton with the file it replaces; retime, trim or copy tracks of an `.ANI` into a new file; export the six formats to JSON and import the JSON back |
| [`svmap`](#shaiya-svmap) | Server map files (`.svmap`): check, dump, build, round trip, compare; check a client's `.wld` files; spawn report; bulk-edit dry run |
| [`clients`](#shaiya-clients) | The saved "my client folders" list (`@name`) |
| [`version`](#shaiya-version) | Version, git revision, target |

Planned: `log`. New to the toolkit? Start with [the five-minute guide](../getting-started.md).

## Global flags

They may come before or after the command.

| Flag | Meaning |
|---|---|
| `--json` | Print **one JSON document** on stdout instead of text. |
| `-q`, `--quiet` | No progress and no notes; errors still go to stderr, and so does the `sdata table` line saying whether the layout is FIT-ONLY or loader-verified. |
| `-v`, `--verbose` | More detail (elapsed times); repeatable. |
| `--progress <auto\|always\|never>` | `auto` shows progress only when stderr is a terminal; `always` prints a line per 10% (useful for logs). |
| `--data-dir DIR` | Keep the saved client folders and settings in `DIR` instead of the default data folder (see [install.md](../install.md#portable-mode-and-where-settings-live)). Shaiya Toolkit takes the same flag. |
| `-h`, `--help` | Help. Every command has one: `shaiya archive extract --help`. |
| `-V`, `--version` | The version line. |

## Conventions (every command follows them)

- **ARCHIVE arguments** (`archive`, `sdata --from`) accept a client folder (the one holding `data.sah`, in any letter case), a `.sah` file (its `.saf` is the file with the same stem next to it), or `@name` from the saved client folders ([`clients`](#shaiya-clients)). Read-only commands that only need the index (`list`, `stats`, `index`, `check`, `diff --quick`) also accept a `.sah` whose `.saf` is missing and say `note: data.saf not found; working from the index only`.
- **CLIENT arguments** (`patch apply`, `patch rollback`, `patch history`, `archive swap`, and the client of [`svmap`](#shaiya-svmap)) accept a client folder or `@name`. Commands that change the archive in place only work on a folder's own `data.sah`/`data.saf`; for any other pair write a new one with `-o`.
- **`@name`** is looked up in the saved list. An unknown name stops with exit 3 and a message ending `(hint: shaiya clients list)`. **In PowerShell a bare `@name` is splatting, not text: write it in double quotes (`"@us"`).** Double quotes work the same in `cmd.exe`, so the pages here use them. Single quotes work in PowerShell only; in `cmd.exe` they are passed on literally and the name is not found. Patterns such as `"*.SData"` are quoted the same way. When a path contains `$` or a backtick, the hints the tool prints use single quotes (the PowerShell form), which `cmd.exe` does not accept.
- **Loose clients.** A saved client can be a *loose data folder* (no `data.sah`; see [loose data folders](../formats/loose-client.md)): `shaiya clients add NAME FOLDER [--backups DIR]` saves it when the folder passes the detection, `clients list` marks it `[loose]` and `clients show` names the layout, the file count, the Shaiya-GX flag and whether a backup folder is confirmed (`--backups` inside the data folder, the backup folder of another saved client or a folder inside another client's data folder is exit 4; a saved client of a kind this version does not know is listed `[unsupported: ...]` and every command that would open it is refused with exit 4). `shaiya anim ... --from "@name"` (or `--from FOLDER`) reads its files; `shaiya info FOLDER` and `shaiya info "@name"` describe it (`Loose client folder, N files; ...`). Every command that opens an archive or a client folder (`archive`, `patch`, `clients health` and `clean`, `sdata --from`) refuses it with exit 2 and `... is a loose folder, not an archive`, saying to use [`shaiya loose`](#shaiya-loose), which lists, changes, patches, undoes and checks it. Tests: `from_a_loose_client_reads_the_file`, `archive_commands_refuse_a_loose_client_exit_2`, `a_backup_folder_inside_data_is_refused_when_adding`, `a_backup_folder_that_another_client_uses_is_refused_and_an_ambiguous_id_is_never_guessed`, `a_saved_client_of_an_unknown_kind_is_listed_unsupported_and_refused_by_every_command`, `a_loose_patch_with_a_required_signature_is_checked_before_anything_is_written`, `loose_replace_then_rollback_restores_the_bytes`.
- **Writing a client in place.** `archive add`, `replace`, `remove`, `compact` and `patch apply` need either `--in-place` (the client's own files; `patch apply` is always in place) or `-o OUT` (a new archive pair, nothing else touched). Giving neither stops with exit 4. `-o OUT` ending in `.sah` names the index (the `.saf` gets the same stem); any other path is a folder that receives `data.sah` and `data.saf`. An existing output stops with exit 4 unless `--force`; an output is never allowed to be one of its inputs.
- **`--dry-run`** prints what would happen, in the shape of the real output, and changes nothing. With `--json` the document has `"dry_run": true`.
- **Backups and journals.** Every in-place change records a *journal* in `<client>/shaiya-toolkit-backups/<UTC stamp>-<label>/` (a copy of the old index, a small `journal.txt`, and a copy of every loose file the change overwrote). `shaiya patch rollback` undoes it. See [undo and recovery](../guides/undo-and-recovery.md) for exactly what is kept and what blocks a rollback.
  - `--journal-dir DIR` puts the journals elsewhere.
  - `--no-journal` skips the journal. The previous index is still copied to `data.sah.bak`, but that is a plain copy that **the next change overwrites**, there is no rollback command for it, and loose files overwritten under `--no-journal` have **no backup at all**.
  - `compact --in-place` is not journaled: it replaces `data.saf` with a rewritten one, keeps only an index backup (`data.sah.bak`), and ends rollback for every earlier journal.
- **In-use check.** Before a write the toolkit tries to open `data.sah`, `data.saf` and `game.exe` for writing. On Windows a running game (or another program holding the files) stops the command: `error: refused: data.saf is open in another program; close the game (and any other program using this client, Shaiya Toolkit included) and try again`, exit 4. On Linux and macOS the toolkit cannot tell and prints `warning: cannot check whether the game is running on this system; make sure it is closed`.
- **Recovery.** Before every write the toolkit finishes or discards the leftovers of an interrupted change and prints each as a `note:` line.
- **Patterns** (`archive list|extract|diff`, `sdata ... --from`): globs, case-insensitive. `*` and `?` stay inside one path segment, `**` as a whole segment crosses segments, `[a-z]` and `[!x]` are character classes, a pattern without `/` matches the file name in any folder, a pattern with `/` is anchored at the archive root, a trailing `/` means everything under that folder, `\` is read as `/`. No regular expressions. `--exclude PAT` (repeatable) removes matches; `--min-size` and `--max-size` take `1500`, `64k` (1000s), `2MiB` (1024s), `1.5MB`, `3G`; `--list FILE` reads paths from a file (one per line; only the first tab-separated column is used, `#` lines and a header line `path` are skipped).
- **Shadowed entries.** When two entries have the same name (any case) in one folder, the client loads the later one; the earlier is *shadowed*. Commands show only visible files; `--include-shadowed` adds the others (extract names them `name~shadowed-N.ext`). See [the archive format](../formats/archive.md#shadowing).
- Archive paths are shown with `/`, matched case-insensitively, and handled in code page 949 in the archive.
- Human output goes to **stdout** (one record per line for listings; summaries go to stderr); progress, notes, warnings and errors go to **stderr** (`error: <message>`, optional `hint: <text>` lines).
- **`--json`**: stdout holds exactly one JSON value, the result object or an error object, for example `{"error": {"kind": "format", "message": "Item.SData: SEED checksum failed (at byte 40)", "file": "Item.SData", "offset": 40}}`. The error kinds are `io`, `format`, `unsupported`, `refused`, `cancelled`, plus `usage` (exit 2) and `internal` (exit 5) from `shaiya svmap`, `shaiya anim export`, `shaiya anim diff` and `shaiya anim import`. Keys are snake_case, sizes are integers in bytes, hashes lower-case hex, times ISO-8601 UTC, archive paths use `/`. Adding a key is not a breaking change; renaming or removing one is (CHANGELOG "Breaking").
- Progress line (stderr, at most 10 updates per second): `[stage]  45%  900.0 MB / 2.0 GB  120.0 MB/s  ETA 8 s`, shown for jobs over 64 MiB or 2 seconds.
- Output files are written atomically (temporary file, then rename), so an interrupted run leaves the target as it was and at most a `*.shaiya-tmp` file.
- On Windows, when `shaiya.exe` is started by double-click, it prints the help and "This is a command-line program. Open a terminal and run `shaiya --help`." and waits for Enter.

### Checks before files go in

`patch build` (the files it would carry), `archive add` and `archive replace` (the files they would put in) look at each file first and print what they find. A finding is a warning: `warning: data/Item.SData: MESSAGE [code]` on stderr (silenced by `-q`) and an entry `{"path", "code", "message"}` in the `warnings` array of the `--json` result. Findings never stop the command and never change an exit code.

| Code | Meaning |
|---|---|
| `sdata-double-encrypted` | A `*.SData` file is SEED-encrypted and its decrypted body starts with the SEED signature again, as if it had been encrypted twice |
| `sdata-crc-bad` | A `*.SData` file is SEED-encrypted but its checksum fails, or its wrapper is cut short or malformed |
| `sdata-state-differs` | A `*.SData` file is plain where the copy already in the client's archive is encrypted, or the reverse (`archive add` and `archive replace` only; a patch has no client copy to compare with) |
| `texture-unreadable` | A `*.dds` or `*.tga` file whose header the toolkit's own texture reader refuses; the game may still accept it |
| `empty-file` | The file is 0 bytes |
| `anim-unreadable` | A `*.ani` file the toolkit's own reader does not read to its last byte; the game may still accept it |
| `anim-invalid` | A `*.ani` file that reads but has validation errors (the [animation checks](../formats/ani.md#what-the-toolkit-checks-validation-codes)): "N validation errors, first: CODE at bone I" |
| `anim-rig-differs` | A `*.ani` file whose skeleton (bone count, parents or reference matrices) differs from the file it replaces; meshes are matched to bones by index, so a replacement must keep the skeleton. `archive add` and `archive replace` only; the Toolkit asks before queueing it, the command line only warns |
| `anim-first-field-differs` | A `*.ani` file whose first field differs from the file it replaces (inferred: the start key; not checked in the game); only a warning |

Only the file name's extension decides which files are looked at (`.SData`, `.dds`, `.tga`, `.ani` in any letter case). The checks read the file once and do not parse SData tables. The rig check compares with the copy already in the client's archive (`archive add` and `archive replace` only; a patch has no client copy, so it gets `anim-unreadable` and `anim-invalid` but no rig check, and a copy that does not read gives no rig check).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success; checks passed; no differences. |
| 1 | The answer is "no": differences found, a check, a CRC or a verification failed. |
| 2 | Usage error (unknown flag, missing argument). |
| 3 | Input error: unreadable or missing file, unknown `@name`, bad format, checksum failure, unsupported version. |
| 4 | Refused by a safety rule: existing output, an output inside a client folder, no `--in-place` or `-o`, the game or another program has the files open, blocked rollback, bad names. |
| 5 | Internal error: a panic, or the one detected bug of `svmap build` (the output did not read back to the same map) or of `anim export` and `anim import` (the output did not read back the same; nothing is written and an existing file is kept). The message asks for a bug report. With `--json` the bug cases print `{"error": {"kind": "internal", "message": ...}}`. |
| 130 | Cancelled. |

A command over several inputs continues past bad inputs, reports each, and exits with the highest code met.

## `shaiya info`

```
shaiya info [--hash] <PATH>...
```

Identifies each file **by content first, name second** and describes it:

```
data.sah  SAH index, version 0, 19,939 files in 166 folders, 0 free entries, data ends at 1,991,954,802
          771,851 bytes
Item.SData  SData, SEED-encrypted, plaintext 4,512,768 bytes, checksum OK
game.exe  Windows program (x86), linked 2010-04-26 03:12:44 UTC, 3,203,072 bytes
```

Kinds: sounds and pictures by content (a WAV: `WAV, PCM 16 bit, 2 channels, 44100 Hz, 1.0 s, N bytes`; an Ogg Vorbis file; a JPEG: `JPEG baseline, 3 components, 1024 x 768, N bytes`; a PNG), from their headers only (see [sounds](../formats/wav.md) and [JPG, PNG and OGG](../formats/jpg-png-ogg.md)); SAH index, SAF data (with the file count and live bytes from a `.sah` next to it), SData (SEED-encrypted, checksum checked; or plain), patch (archive files, loose files, removals, whether a `.sig` sits next to it), patch signature, model lists (a file named `.mlt`, `.mon` or `.itm` is read as that list: `MLT list, N records, ...`; one that starts with another signature, such as `MO4`, is unknown with a `problem:` line saying it is a layout of another episode, not read), Windows program (machine and link time), unknown (the first 16 bytes). A damaged file is reported with a `problem:` line, not a failure; files over 512 MiB are not decrypted for the checksum check.

`--hash` adds a `sha256 <64 hex digits>` line per file and runs with progress.

Exit 0 when every path was read and none has problems; 3 when a path could not be read or has problems. With `--json` the result is `{"files": [ ... ]}` with one object per path (`path`, `size`, `kind`, `details`, `sha256`, `problems`); paths that could not be read at all are listed under `errors`.

## `shaiya archive`

| Command | Arguments and flags | Exit |
|---|---|---|
| `list` | `ARCHIVE [PATTERN]...` `--exclude PAT` `--list FILE` `--min-size N` `--max-size N` `--include-shadowed` `--long` `--tree` `--folders` `--names-only` `--sort name\|size\|offset\|version` `--reverse` | 0; 3 |
| `stats` | `ARCHIVE` `--top N` (rows per table, default 15) | 0; 3 |
| `check` | `ARCHIVE` `--deep` (read every visible file) `--strict` (warnings count as problems) `--gaps N` (largest gaps to print, default 10) | 0 no problems; 1 problems; 3 |
| `index` | `SAH` | 0 rewrite identical; 1 not identical; 3 |
| `extract` | `ARCHIVE [PATTERN]... -o DIR` `--exclude PAT` `--list FILE` `--min-size` `--max-size` `--include-shadowed` `--force` or `--skip-existing` `--no-times` `--dry-run` | 0; 3; 4 targets exist |
| `cat` | `ARCHIVE PATH` `-o FILE` `--force` | 0; 3; 4 |
| `add` | `ARCHIVE SRC...` (`--in-place` or `-o OUT`) `--at PREFIX` `--as PATH` `--replace` `--timestamp T` `--dry-run` `--force` `--no-journal` `--journal-dir DIR` | 0; 3; 4 |
| `replace` | as `add`, with `--add-missing` instead of `--replace` | 0; 3; 4 |
| `remove` | `ARCHIVE [PATH]...` (`--in-place` or `-o OUT`) `--list FILE` `--ignore-missing` `--dry-run` `--force` `--no-journal` `--journal-dir DIR` | 0; 3; 4 |
| `compact` | `ARCHIVE` (`--in-place` or `-o OUT`) `--min-dead N` `--dry-run` `--force` `--journal-dir DIR` | 0; 3; 4 |
| `diff` | `OLD NEW [PATTERN]...` `--quick` `--include-shadowed` `--only added,removed,different,unknown` `--show-same` `--summary` | 0 same; 1 differences; 3 |
| `pack` | `FOLDER -o OUT` `--sah-version 0\|1` `--timestamp T` `--include-all` `--exclude PAT` `--force` `--dry-run` | 0; 3; 4 |
| `swap` | `CLIENT OTHER` `--dry-run` | 0; 3; 4 |

Details:

- **`list`**: one line per file: size then path (`--long`: offset in hex, size, pack time, path; `--names-only`: just paths; `--tree` groups by folder; `--folders` shows folders only with file counts and sizes). The summary (`9 of 19,939 files, 6.6 MB`) goes to stderr.
- **`stats`**: SAH version, file counts (visible, shadowed), folders, free-list entries, trailer, `data.saf` size, live and unused bytes, and tables by folder and by extension.
- **`check`** findings: `out_of_bounds` (problem); `unreadable_entry` (problem, `--deep`); `overlap` (warning for a partial overlap, info for an exactly shared range); `shadowed` (warning, one per shadowed entry, names the visible one); `hash_collision` (warning: different names, same lookup hash, which the client also shadows); `undecodable_name` (warning: not valid code page 949); `escaped_name` (info: would be renamed on extract); `zero_length`, `dead_space`, `free_list`, `saf_missing`, `trailer` (info). Ends with `OK: 0 problems, 0 warnings` or the counts. `--strict` turns warnings into problems for the exit code.
- **`index`**: parses `data.sah`, writes it back in memory and compares with the file.
- **`extract`**: folders are created; existing files stop the command with exit 4 unless `--force` (overwrite) or `--skip-existing`. File times are set from the pack time unless `--no-times`. Names Windows cannot hold are changed (a `:` becomes `%3A`, reserved names like `CON` get escaped) and are never written outside `-o`. A failed entry is reported and the rest continue (exit 3).
- **`cat`**: writes one file to `-o FILE`, or to stdout when stdout is not a terminal; to a terminal without `-o` it stops with exit 4.
- **`add` / `replace`**: `SRC` is files and folders. A file goes to `PREFIX/<file name>`; a folder's files go to `PREFIX/<path relative to that folder>` (the folder's own name is not a path segment). `--as PATH` gives a single file's exact archive path. The entry's time is the file's modification time or `--timestamp` (Unix seconds, `2026-11-15` or `2026-11-15T12:00:00Z`). `add` refuses paths that already exist unless `--replace`; `replace` refuses paths that do not exist unless `--add-missing`. Refusals list every path at once (exit 4). Names must be encodable in code page 949. The files are checked first and findings are printed as warnings (see [Checks before files go in](#checks-before-files-go-in)); an SData file is also compared with the copy the archive already holds at that path.
- **`remove`**: paths as arguments and/or `--list FILE`. A path that is not in the archive stops the command unless `--ignore-missing` (those are reported as "not present").
- **In place** (`--in-place`): the new bytes are appended to `data.saf`, the index is replaced last, and a journal is written (see conventions). `-o OUT` instead writes a fresh, compacted pair (shadowed entries and the free list are dropped) and leaves the client alone.
- **`compact`**: rewrites `data.saf` with only the bytes the index uses, drops shadowed entries and the free list. `--dry-run` prints size, live and unused bytes, and the free space needed (live bytes plus 256 MB). `--min-dead N` does nothing when fewer than N bytes are unused. **In place, it ends rollback** of every earlier journal of that client; the command says how many ("N earlier changes can no longer be rolled back after compacting"). `-o OUT` writes a compacted copy and changes nothing else.
- **`diff`**: `--quick` compares indexes only (equal length and pack time count as the same); otherwise contents are compared (a side without `data.saf` switches to `--quick` with a note). Lines are `+ added`, `- removed`, `~ different`, `? unknown` (and `= same` with `--show-same`); `--summary` prints only the counts line.
- **`pack`**: builds a new pair from a folder (sorted, names in code page 949; `Thumbs.db`, `desktop.ini` and similar are skipped unless `--include-all`). `--sah-version 1` writes the version-1 index.
- **`swap`**: puts another archive pair (a folder or a `.sah`) in place of the client's own. The old pair is kept as `data.<UTC stamp>.sah` / `.saf` in the same folder, and the command prints the `swap` command that goes back. `--dry-run` prints the four renames. It is crash safe (see [undo and recovery](../guides/undo-and-recovery.md#swap-back)).

Output shapes (numbers illustrative):

```
$ shaiya archive list "@us" "*.SData" --long
0x00a1b2c3d4   2,960,114  2010-05-17T03:12:44Z  Item/Item.SData

$ shaiya archive check "@us"
data.sah  version 0, 19,939 files, 0 shadowed, 166 folders, no free list
data.saf  1,991,954,802 bytes, all referenced, 0 gaps, 0 overlaps
info      zero_length: 0 files
info      dead_space: 0 B unused (0.0 %) in 0 gaps
info      trailer: 00000000
OK: 0 problems, 0 warnings

$ shaiya archive add "@us" mods/ --in-place --replace
added 1, replaced 1, removed 0 (2.9 MB appended to data.saf)
  + interface/t1.txt
  ~ Item/Item.SData
backup: C:\Games\Shaiya\shaiya-toolkit-backups\20261115T120000Z-archive-edit-2-files  (undo: shaiya patch rollback "@us")

$ shaiya archive diff data.orig.sah data.sah --quick --summary
476 removed, 8 added, 0 different, 19,931 same, 0 unknown
```

JSON (`--json`):

| Command | Document |
|---|---|
| `list` | `{"archive": {"sah", "saf" (or null), "sah_version", "files"}, "entries": [{"path", "size", "offset", "version", "version_utc", "shadowed", "raw_hex" (only for names that are not valid code page 949)}], "shown", "shadowed_hidden", "total_size"}`; `--tree` and `--folders` add `"folders": [{"path", "files", "bytes"}]` |
| `stats` | `{"sah_version", "files", "visible", "shadowed", "dirs", "free_entries", "trailer_hex", "data_end", "saf_len", "live_bytes", "dead_bytes", "zero_length", "by_folder": [{"name", "files", "bytes"}], "by_extension": [{"ext", "files", "bytes"}]}` |
| `check` | `{"ok", "problems", "warnings", "sah_version", "files", "visible", "dirs", "free_entries", "trailer", "layout": {"saf_len", "data_end", "live_bytes", "dead_bytes", "gap_count", "largest_gaps", "overlaps", "shared_ranges", "zero_length", "out_of_bounds", "out_of_bounds_count"}, "findings": [{"code", "severity", "message", "path"}]}` |
| `index` | `{"sah", "sah_version", "files", "free_entries", "trailer_hex", "identical", "first_difference"}` |
| `extract` | the extract report (written, skipped, escaped names, bytes) plus `"out"`; `cat -o`: `{"path", "size", "out"}` |
| `add`, `replace`, `remove` | `{"added": [...], "replaced": [...], "removed": [...], "missing": [...], "loose": [...], "bytes_appended", "saf_len_before", "saf_len_after", "journal", "warnings"}`; with `-o` also `"out_sah"`, `"out_saf"`; dry run adds `"dry_run": true` |
| `compact` | in place: `{"before", "after", "duplicates": [...]}`; dry run: `{"dry_run", "nothing_to_do", "size", "live_bytes", "dead_bytes", "shadowed", "free_entries", "needs_free_bytes"}`; with `-o` as `add` |
| `diff` | `{"summary": {"added", "removed", "different", "same", "unknown"}, "entries": [...]}` (entries empty with `--summary`) |
| `pack` | the pack report plus `"out_sah"`, `"out_saf"` |
| `swap` | `{"installed_from", "kept_as": [sah, saf]}` |

## `shaiya patch`

| Command | Arguments and flags | Exit |
|---|---|---|
| `build` | `-o OUT.patch` `--files DIR` `--loose DIR` `--remove LIST` `--diff OLD NEW` `--only-changed CLIENT` `--timestamp T` `--sign KEY --check-version N` `--force` `--dry-run` | 0; 3; 4 |
| `list` | `PATCH` | 0; 3 |
| `verify` | `PATCH...` `--key PUBLIC.key` or `--public-hex HEX` with `--check-version N` `--client CLIENT` `--require-signature` | 0; 1 a check failed; 3; 4 `--require-signature` without a key, or a key without `--check-version` |
| `apply` | `CLIENT PATCH...` `--dry-run` `--key PUBLIC.key` or `--public-hex HEX` with `--check-version N` `--protect NAME` `--no-journal` `--journal-dir DIR` | 0; 3; 4 |
| `publish` | `PATCH...` `--to DIR` `--check-version N` `--updater-version N` `--new-check-version` `--layout shaiya\|flat` `--dry-run` | 0; 3; 4 |
| `rollback` | `CLIENT` `--id ID` `--force` `--dry-run` `--journal-dir DIR` | 0; 3; 4 blocked |
| `history` | `CLIENT` `--prune KEEP` `--journal-dir DIR` | 0; 3 |
| `keygen` | `--secret FILE --public FILE` | 0; 3; 4 a file exists |
| `sign` | `FILE... --key SECRET.key --check-version N` `--force` | 0; 3; 4 a `.sig` exists |
| `sign-updater` | `EXE --key SECRET.key --updater-version V --check-version N` `--force` | 0; 3; 4 |

- **`build`** inputs: `--files DIR` mirrors archive paths (`DIR/Item/Item.SData` becomes `Item/Item.SData`); `--loose DIR` holds files for the client folder itself (for example `game.exe`); `--remove LIST` names archive paths to delete (one per line, same format as `--list`); `--diff OLD NEW` builds the patch that turns archive OLD into archive NEW (instead of `--files`/`--remove`; it may be combined with `--loose`). `--only-changed CLIENT` (with `--files`) leaves out every archive file that is already in that client's archive with the same length and the same SHA-256, lists them on stderr (`left out, already the same in the client`) and in the JSON `unchanged` array; the client is only read, and a folder where nothing differs builds an empty patch with the usual "the patch is empty" warning. The same inputs always give the same bytes (file order, times and compression are fixed; `--timestamp` sets the entry time of every file, otherwise file times are used). The build warns on stderr (it never fails for these) when the output name is not `psNNNN.patch` (the original updater only fetches that name), when the patch carries removals (`remove.txt` is honoured by the toolkit and the replacement updater, not by the original `Updater.exe`), and when it is empty. Names that cannot be written (unencodable in code page 949, reserved toolkit names, `..` paths, names differing only by case) stop the build with exit 4 and list every offender. Every SData, `.dds` and `.tga` file and every empty file is also checked and each finding is a `warning: PATH: MESSAGE [code]` line on stderr and an entry of the JSON `warnings` array (see [Checks before files go in](#checks-before-files-go-in)); findings never change the exit code. `--sign KEY --check-version N` also writes `OUT.patch.sig`. `--dry-run` prints the contents table and sizes.
- **`list`**: prints what is inside (`archive`, `loose`, `remove` lines) and a count line.
- **`verify`**: one block per patch: structure (ZIP opens, `update.sah` parses, `update.saf` covers every entry, every entry reads to its end, removal and loose names valid, no case duplicates) and signature (`valid`, `MISSING`, `invalid`, or "not checked" without a key). A key needs `--check-version`. `--require-signature` makes a missing `.sig` a failure. `--client CLIENT` adds whether the patch is **applied**: every file present and equal, every removal absent, every loose file equal by SHA-256 (`applied to the client: yes` or a list of what differs).
- **`apply`**: patches are applied in the order given (a warning when the `psNNNN` numbers are not ascending), each with its own journal. With `--key` (or `--public-hex`) and `--check-version`, **every** patch's signature is verified before the first write (exit 4 on failure); without a key the output says `signature not checked`. A failure stops at that patch; earlier ones stay applied and are listed. `--protect NAME` writes a loose file of that name as `NAME.new` instead of overwriting it. `--dry-run` shows, per patch, what would be added, replaced, removed, written, how many bytes are appended and how much free space is needed.
- **`publish`**: copies `psNNNN.patch` files into a server folder and raises `PatchFileVersion` in its `UpdateVersion.ini` ([the file](../formats/version-ini.md)). `--layout shaiya` (default) uses `DIR/Shaiya/UpdateVersion.ini` and `DIR/Shaiya/patch/`; `flat` uses `DIR` for both. A new ini needs `--check-version N`; an existing one keeps its own value, and a different `--check-version` needs `--new-check-version`. Refused with exit 4 before anything is written: a name that is not `psNNNN.patch` (four digits), a patch that fails the structure checks, a gap after `PatchFileVersion`, a different file under a name already in the folder, a `CheckVersion` change without the flag, an unreadable existing ini, and a destination inside a saved client or a folder holding `data.sah` and `data.saf` (also when the client is where the files would go, such as `DIR/Shaiya`). A change found during the write (a patch that appeared in the folder, the ini changed meanwhile) is exit 4 too and may come after some patches were copied; the message lists them and says the ini was not changed. A missing patch file is exit 3. Publishing the same patches again is a no-op (exit 0). The patches' files get the [checks before files go in](#checks-before-files-go-in) as `warning:` lines, plus a warning for removals (the original updater ignores them). `--dry-run` prints the plan and writes nothing; a real run reads every written file back.
- **`rollback`**: undoes the newest change (or `--id ID`; ids are in `history`). It restores the old index, cuts `data.saf` back to its earlier length, restores the loose files the change overwrote and deletes the ones it created. A loose file that changed after the patch blocks it unless `--force` (the changed file is saved first). It is blocked, with the reason printed, by a newer journal, a compaction, a swap or any other change of `data.sah`; see [undo and recovery](../guides/undo-and-recovery.md#what-blocks-a-rollback). Exit 4 when blocked.
- **`history`**: a table of id, label, kind (`patch` or `archive-edit`), created, state (`applied`, `rolled back <time>`, `pending`) and whether rollback is possible. `--prune KEEP` deletes all but the newest KEEP journals (rolling back a pruned journal is then impossible).
- **`keygen`**: writes an Ed25519 key pair (hex text files), refuses to overwrite either, prints the public key and reminders: keep the secret key off the patch server and backed up (on Windows: in a folder only you can open), and never sign a different file under a patch number you have already published.
- **`sign`**: writes `FILE.sig` next to each file. The signature covers the file name, the CheckVersion and the file's SHA-256 ([patch format](../formats/patch.md#signatures)).
- **`sign-updater`**: signs an updater executable under `shaiya-updater-<V>.exe`.

```
$ shaiya patch list ps0109.patch
ps0109.patch  ZIP 1.9 MB (3.0 MB unpacked), update.sah version 0, signature file present
  archive   2,960,114  2010-05-17T03:12:44Z  Item/Item.SData
  loose        12,345                        notice.txt
  remove                                     test/junk.eft
1 archive file (2.9 MB), 1 loose file, 1 removal

$ shaiya patch apply "@us" ps0109.patch --dry-run
ps0109.patch -> C:\Games\Shaiya (data.sah, 19,939 files)
  would add 0, replace 1, remove 1 (0 not present), write 1 loose file (overwrites notice.txt)
  would append 2.9 MB to data.saf; needs about 5.8 MB free on that drive while applying
  signature not checked

$ shaiya patch verify ps0109.patch ps0110.patch --key public.key --check-version 3
ps0109.patch  structure OK (1 archive file, 0 loose files, 0 removals); signature valid
ps0110.patch  structure OK (4 archive files, 0 loose files, 0 removals); signature MISSING (ps0110.patch.sig not found)

$ shaiya patch rollback "@us"
rolled back ps0109.patch (applied 2026-11-15T12:00:00Z): index restored, data.saf truncated to 1,991,954,802 bytes, 1 loose file restored, 0 deleted
```

JSON: `build` gives `{"out", "files": [...], "loose": [...], "removals": [...], "archive_bytes", "loose_bytes", "signature", "dry_run", "unchanged", "warnings": [{"path", "code", "message"}]}`; `list` gives `{"files": [{"path", "length", "version"}], "loose": [[name, size]], "removals": [...], "sah_version", "signature_file", "zip_size", "unpacked_size"}`; `verify` gives `{"patches": [{"patch", "problems": [...], "signature": {"status"}, "not_applied": null or [...]}]}`; `apply` gives `{"client", "results": [{"added", "replaced", "removed", "missing", "loose", "bytes", "journal"}]}` (dry run: `{"dry_run": true, "plans": [{"patch", "added", "replaced", "removed", "missing", "loose": [{"name", "target", "size", "overwrites", "protected"}], "bytes", "temp_bytes"}]}`); `publish` gives `{"dry_run", "plan": {"layout", "dir", "ini_path", "patch_dir", "items": [{"number", "patch": {"source", "target", "state", "size", "sha256"}, "signature", "has_removals"}], "ini_before", "check_version", "updater_version", "patch_file_version_before", "patch_file_version_after", "check_version_changed", "updater_version_changed", "updater_version_defaulted", "ini_changed", "warnings"}, "warnings": [{"path", "code", "message"}], "notes", "nothing_to_do", "result": {"copied", "already_published", "ini_written", "patch_file_version", "read_back"}}` (`result` only without `--dry-run`); `rollback` gives `{"journal", "restored_index", "truncated_to", "restored_loose", "deleted_loose"}`; `history` gives `{"journals": [{"id", "dir", "kind", "label", "created_utc", "tool", "sah_name", "saf_name", "saf_len_before", "saf_len_after", "sah_sha256_before", "sah_sha256_after", "loose": [...], "state", "rollback": {"possible", "reason"}}], "pruned": [...]}`; `keygen` gives `{"public_hex", "secret_file", "public_file"}`; `sign` and `sign-updater` give `{"signed": [{"file", "sig", "name", "check_version"}]}`.

## `shaiya sdata`

| Command | Arguments and flags | Exit |
|---|---|---|
| `info` | `INPUT...` or `--from ARCHIVE [PATTERN]...` `--all-files` | 0; 3 |
| `check` | same as `info` | 0 all OK or plain; 1 a CRC failed; 3 |
| `decrypt` | `INPUT...` (`-o PATH` or `--in-place`) or `--from ARCHIVE [PATTERN]... -o PATH` `--unchecked` `--skip-plain` `--force` or `--skip-existing` `--all-files` `--dry-run` | 0; 1 CRC failed with `--unchecked`; 3; 4 |
| `encrypt` | `INPUT...` (`-o PATH` or `--in-place`) `--like PATH\|ARCHIVE` `--skip-encrypted` `--force` or `--skip-existing` `--all-files` `--dry-run` | 0; 3; 4 |
| `table` | `INPUT` or `--from ARCHIVE INPUT`, `--layout auto\|item\|skill\|monster\|npcquest`, `--section NAME`, `--csv OUT` (`--force`), `--code-page cp949\|cp1252\|cp1251\|cp936\|utf-8\|utf-16le` | 0; 3 unreadable, or no layout (or more than one) reads the whole file; 4 `--csv OUT` exists without `--force`, or `--section` names no table |

- Inputs are files, or folders searched recursively for names ending in `.SData` (any case; every file with `--all-files`). With `--from`, the inputs are patterns over the archive's visible files (default `*.sdata`).
- `-o PATH` is a file when there is exactly one input and `PATH` is not an existing folder and does not end in `/` or `\`; otherwise it is a folder (created) that receives each file under its own relative name. `-o` equal to an input folder without `--in-place` stops with exit 4. Existing outputs stop the command (exit 4) unless `--force` or `--skip-existing`.
- `--in-place` replaces the input after copying the original to `NAME.bak` (the note says so; an existing `.bak` is not overwritten, a time-stamped name is used instead). Do not use it on files inside a client's archive: use `--from` with `-o`, then put the result back with `archive replace` or a patch.
- `decrypt` copies a plain (unencrypted) file unchanged (status "plain, copied") unless `--skip-plain`. A file whose CRC fails is exit 3 for that file and the batch continues, unless `--unchecked` (written anyway, status "checksum mismatch", exit 1).
- `encrypt` copies an already encrypted file unless `--skip-encrypted`. `--like` takes a file (one input), a folder of originals or an ARCHIVE: each output reuses the original's header padding and tail, so an unchanged file comes out byte-identical, and a plain original stays plain.
- The summary line (`9 files: 6 done, 3 copied, 0 skipped, 0 failed`) goes to stderr.
- `table` reads one file (encrypted or plain) as rows and columns, read only. A layout is used only when it reads the whole file to its last byte with plausible counts; `--layout auto` tries every layout and uses the only one that fits. Without `--csv` or `--json` it prints the layout, a line saying it was chosen because it reads the whole file (checked against the game's own loader only on the clients listed as verified in the version matrix), the tables and their columns; the JSON (and the `--csv --json` summary) has `"chosen_by": "fit"`. Where the layout comes from is on stderr (also with `--csv` and with `--quiet`): for a layout read from a client's own loader "Loader-verified on: <clients>." (and, when it also fits other clients without a loader read, which), for a FIT-ONLY layout the sentence "FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly." The JSON adds `"status"` (`"loader-verified"` or `"fit-only"`), `"loader_verified_on"` and `"fit_only_on"` (lists of client ids; the ids are the [version matrix](../version-matrix.md) columns). `status` describes the layout, not the file's client: a loader-verified layout is still FIT-ONLY on the clients in `fit_only_on`, and the command does not know which client a file came from. `--csv OUT` writes the main table (or the one `--section` names) as UTF-8 with a byte-order mark, CRLF line ends and commas; a text cell starting with `=`, `+`, `-`, `@`, a tab or a carriage return gets a leading `'`, numbers are not touched. A file with several tables (`NpcQuest.SData`: `merchants`, `gatekeepers`, `npcs`, `quest_links`, `quests`) writes `quests` and says so on stderr. Names and descriptions are decoded with `--code-page` (default `cp949`). The layouts and what they were checked on: [Item.SData](../formats/item-sdata.md), [Skill.SData](../formats/skill-sdata.md), [Monster.SData](../formats/monster.md), [NpcQuest.SData](../formats/npcquest.md).

```
$ shaiya sdata info Item.SData GuildHouse.SData
Item.SData        SEED, plaintext 2,960,114 bytes, CRC 0x8c1f02aa OK, header padding zero, tail zero
GuildHouse.SData  not encrypted, 41,220 bytes

$ shaiya sdata decrypt --from "@us" -o plain/
$ shaiya sdata encrypt plain/ -o enc/ --like "@us"
```

JSON: `{"items": [{"name", "status", "size", "plain_size", "crc", "crc_ok", "padding_zero", "tail_zero", "out", "message"}], "summary": {"<status>": count}}`. For `table`: `{"layout", "table", "chosen_by", "status", "loader_verified_on", "fit_only_on", "sheets": [{"name", "columns": [{"name", "kind"}], "row_count", "rows": [[...]]}]}` (`kind` is `int`, `float` or `text`; `--section` keeps one sheet); with `--csv` and `--json` together, `{"layout", "table", "chosen_by", "status", "loader_verified_on", "fit_only_on", "section", "rows", "columns", "out"}`.

Guide: [SData files](../guides/sdata-files.md).

## `shaiya anim`

Animation and model files: `.ANI` (skeletal animations), `.3DC` (the skinned meshes they move), `.VANI`, `.MANI`, `.3DO` and `.3DE`. `export` and `import` write only the `-o` file (a new file; an existing one only with `--force`), never inside a client folder; the other commands only read. The formats are in [formats/ani.md](../formats/ani.md), [formats/3dc.md](../formats/3dc.md), [formats/vani.md](../formats/vani.md), [formats/mani.md](../formats/mani.md), [formats/3do.md](../formats/3do.md) and [formats/3de.md](../formats/3de.md). `info` and `export` read all six; `validate` and `compare` read `.ANI` files only and say so when given another format. A file is described only when it reads to its last byte; otherwise the command says so and exits 3. Every file was read on US EP4.5 only; the eight other matrix clients have their index only.

| Command | Arguments and flags | Exit |
|---|---|---|
| `info` | `INPUT...` or `--from ARCHIVE [PATTERN]...` (all six formats) | 0; 3 a file does not read or is over 128 MiB (the others are still reported) |
| `validate` | same as `info`; `.ANI` only | 0 no file has an error (warnings are allowed); 1 a file has an error; 3 a file does not read or is over 128 MiB |
| `compare` | `CANDIDATE TARGET` or `CANDIDATE TARGET --from ARCHIVE` | 0 the candidate passes; 1 it does not; 3 a file does not read, a name that is not an `.ANI` (refused by its extension before it is read), `TARGET` is not in the archive, or a file is over 128 MiB |
| `export` | `INPUT -o OUT.json` or `INPUT --from ARCHIVE -o OUT.json`, `--force` (all six formats); or `INPUT... --out-dir DIR` (see below) | 0 written; 2 `-o` does not end in `.json`; 3 the file does not read (a `.3DE` the reader refuses, a `.ANI` key that is NaN or infinite), is not one of the six, is not in the archive, is over 128 MiB, or the `-o` folder does not exist; 4 the output exists (without `--force`), is inside a client folder, or is the input; 5 a bug (the JSON did not read back to the same bytes; nothing is written) |
| `diff` | `A B`, `--from ARCHIVE` (where `A` and, unless `--from-b` is given, `B` are), `--from-b ARCHIVE` (where `B` is); `.ANI` only | 0 the skeletons, the end keys and the poses are identical; 1 anything differs; 3 a file does not read, is not an `.ANI` (refused by its extension before it is read), is not in the archive or is over 128 MiB |
| `import` | `IN.json -o OUT`, `--like TARGET [--from ARCHIVE]` (required for `.ANI`), `--force`; or `IN.gltf` or `IN.glb` with `-o` ending in `.ani`, `.3dc`, `.3do` or `.3de`, and `--like` (required for `.ANI` and `.3DC`), `--map FILE`, `--reduce-influences`, `--reduce`, `--keys exact` (see the bullet below); or `IN.obj` with `-o` ending in `.3do` or `.3de`; or `IN.bvh` with `-o` ending in `.ani` and `--like` (required) | 0 written; 1 an `.ANI` that does not pass against `--like` (nothing written); 2 `--like` missing for an `.ANI`, `--like` for another format, `--from` without `--like`; 3 the JSON is malformed, unknown or newer, holds a refused value or a count over a limit, or is over 256 MiB, the `-o` extension is not the JSON's format, a file over 128 MiB, the `-o` folder does not exist; 4 as `export`; 5 a bug (the file written did not read back the same; nothing is written and an existing file is kept) |

- Inputs are files, or folders searched recursively for names ending in `.ani`, `.3dc`, `.vani`, `.mani`, `.3do` or `.3de` in any letter case (`ARM.ANI`, `arm.Ani` and `arm.3dc` are all found; `validate` looks for `.ani` only). With `--from`, the inputs are patterns over the archive's visible files (default: the six extensions for `info`, `*.ani` for `validate`); only files of those formats that match are described (`.ani` only for `validate`). A file named on the command line that is not one of the six formats (or, for `validate` and `compare`, not an `.ANI`) is reported as `FAILED` with exit 3 without being read; a folder that cannot be listed is reported as `FAILED` too and the walk goes on to the other folders.
- `info` prints one line per file, for example `Character/Human/ANI/huwm_000_x.ANI: animation, Character rig huwm (Human), 65 bones, end key 240 (8.0 s), 2,210 rotation keys, 130 translation keys, 0 errors, 1 warning` or `Mob.3DC: skinned mesh, 1,234 vertices, 2,100 triangles, 65 bones, highest vertex bone 64`; a `.VANI` as `PATH: vertex-animated model, 1 item, 8 vertices per step, 12 triangles, 12 steps, about 66 ms per step (inferred)`, a `.MANI` as `PATH: building motion, 108 bytes, 20 fields (every meaning inferred), tick 33 ms`, a `.3DO` as `PATH: 3DO mesh, 8 vertices, 12 triangles, 11 frames at 20 per second (inferred)` (the frames part only when the file has frames) and a `.3DE` as `PATH: effect mesh, 8 vertices, 12 triangles, 4 frames` or `..., static, no frame block`. The kind and rig come from an archive path only (`--from`); a file on disk gets neither. Seconds are the end key at 30 key units per second. A first field other than 0 is shown as `first field -1 (inferred: start key)`. A cloak mesh is shown as `cloak mesh without bones`.
- `validate` prints each finding on stderr as `error: PATH: CODE: message` or `warning: PATH: CODE: message` (warnings are hidden by `--quiet`, errors never), then a summary line. The codes are the ones in [formats/ani.md](../formats/ani.md#what-the-toolkit-checks-validation-codes).
- `compare` checks a candidate `.ANI` against the file it would replace. It prints `same skeleton as TARGET: 65 bones, parents and reference matrices identical`, or `the skeleton differs from TARGET:` with each difference (the first 10 of each kind, then `and N more reference matrix differences`); then its own line for the first field, `first field: same (0)` or `first field differs: candidate 0, target -1 (inferred: the start key; not checked in the game)`; then the candidate's validation errors; and last `passes` or `does not pass`. The candidate passes when its skeleton (bone count, every parent, every reference matrix bit for bit) is the same as the target's, it has the same first field (inferred to be the start key; not checked in the game) and it has no validation error. The first field is not part of the skeleton: it is reported as its own difference and `same skeleton` can be printed above a `does not pass`.
- Validated offline only; the commands say nothing about what the game does with a file.
- `export` turns a file into a JSON text (the schemas are in the format pages: [ani](../formats/ani.md#json-export-and-import), [3dc](../formats/3dc.md), [vani](../formats/vani.md), [mani](../formats/mani.md), [3do](../formats/3do.md), [3de](../formats/3de.md)). The format comes from the extension of `INPUT` (any letter case); `-o` must end in `.json`. `INPUT` is one file on disk, or with `--from` one exact path in the archive (not a pattern). The JSON's `source` is the archive path, or the file name only for a file on disk: it never holds a folder path. The file is read only when it is 128 MiB or less, and it must read to its last byte. The text is read back and compared with the file before anything is written, and the file written is read back again before it replaces anything.
- `export --out-dir DIR` writes one JSON per model file instead of one `-o` file: the inputs are files and folders on disk, or with `--from` patterns over the archive's visible files (a pattern that ends in `/` means everything under that folder). Each file becomes `DIR/<path>.json`, with the archive path (or the path inside a searched folder, or the file name of a file named on the command line) kept and `.json` added to the file name, so `huwm_1.ANI` gives `huwm_1.ANI.json`; folders are made as needed. `-o` and `--out-dir` cannot be used together (exit 2), and more than one input needs `--out-dir` (exit 2). `DIR` is checked before anything is read: inside a client folder (a folder with `data.sah`, a saved client folder, or below either, also through a link) it is exit 4 and nothing is made. Each file is exported and written the way `-o` does it (the JSON read back and compared, the file written beside and checked, then renamed). A file that does not read, a name the archive writers refuse (a path part named `data.sah`, a `..` part, a name the toolkit uses for its own files), a name Windows would change (a path part with a trailing dot or space, so `data.sah.` too, a reserved device name such as `CON` or `NUL`, a forbidden character; refused on every system), an output that exists without `--force`, or two inputs with the same output name (compared after Unicode lower-casing: `A.json` and `a.json` are one file on Windows) is reported as `PATH: FAILED: reason` on stderr and the rest go on. The last line on stdout is `wrote N of M (K failed)`. The exit code at the end is the largest of 3 (a file did not read, or its output name is refused), 4 (an output exists, a second input has the same output name, or a folder would be inside a client folder or cannot be made) and 5 (a bug), else 0. With `--json` the output is `{"written": ["DIR/..json", ...], "failed": [{"input", "reason"}], "cancelled": false}`.
- `diff A B` compares two `.ANI` files: it runs the skeleton check of `compare` (bone count, parents, reference matrices, first field) between `A` (as the candidate) and `B`, compares the end keys, and then compares the pose of every bone both files have at every whole key from the smaller first field (or 0) to the larger end key (a very long range is sampled with a stride: at most 100,001 keys, the last being the end key, and keys times bones kept under 20 million). A bone is listed when its world rotation turns by more than 0.001 degrees, or its world position moves by more than 0.0001 units, at some sampled key. The text says `identical: ...` (exit 0), or lists the skeleton differences (the first 10 of each kind), the first field, the end keys, and then `N bones differ in pose (largest turn first):` with at most 20 lines `bone 2: up to 45.00 degrees, 0.414 units (2 keys in A, 2 in B)`, then `and N more`. Swapping `A` and `B` swaps the pairs and keeps every number. A pose is the toolkit's own computation (see [formats/ani.md](../formats/ani.md)); the command says nothing about what the game does. With `--json`: `{"a", "b", "identical", "skeleton": [{"kind", "skeleton", "bone", "candidate", "target"}], "end_key": {"a", "b"}, "bones": [{"bone", "max_angle_deg", "max_offset", "keys": {"a", "b"}}]}` (`candidate` is `A`, `target` is `B`; an infinite value is `null`); a file that does not read prints `{"a", "b", "error"}` and exits 3.
- `import` turns a JSON made by `export` back into a file. The format comes from the JSON's `format` key and `-o` must have that format's extension (any letter case). An unedited JSON gives back the exact bytes of the file it was made from. Unknown keys and other columns are refused.
- `--like TARGET` is required for an `.ANI` and refused for the other five formats; `--from` says that `TARGET` is a path in an archive. The `.ANI` is written only when it passes the same check as `compare` (same skeleton, same first field (inferred: the start key; not checked in the game), no validation error); the import prints the lines `compare` prints, then `nothing written` and exits 1 when it does not pass. The other formats have no such check.
- `edit retime IN --factor F | --seconds S`, `edit trim IN --start K --end K` and `edit copy-track IN --source SRC (--bones 3,4,5 | --all) [--rotation] [--translation]` make a new `.ANI` from `IN` (a file on disk, or with `--from` its path in the archive; `SRC` is read from the same place) and write it to `-o`. `retime` makes the animation `F` times as long (0.1 to 10) or `S` seconds long (30 keys are one second): the new end key is the first field plus the rounded old length times `F`, and every bone that has a track is sampled at every whole key of the new range. The new key `first + i` is sampled at the old time `first + i * length / new length`, so the first and the last pose of the original are the first and the last pose of the result even when the rounding changes the length (30 keys times 0.3 is 9 keys, and key 9 is the old key 30); the motion between is stretched by the rounded factor, and detail between two whole keys of the new range is not kept. Most keys that a straight line (rotation: slerp) reproduces within 1e-5 per component of the track's own value are dropped (a run of dropped keys is at most 64 keys long, so a long straight run keeps one key in 64); a bone without a track stays without one. A result whose file would be over 128 MiB (the most the toolkit reads back) is refused. `trim` keeps the keys `START` to `END` (first field <= `START` < `END` <= end key), samples every track at both ends and moves the part to the first field. `copy-track` copies the rotation and/or translation tracks of the named bones from `SRC` into `IN`; `SRC` and `IN` must have the same skeleton and first field, and the end key becomes the larger of `IN`'s end key and the last key of a copied track (the end key of `SRC` itself is not used, and a shorter copy never shortens the animation). The result is written only when it passes the same check as `compare` against `IN`; the output rules are those of `import`: a new file (an existing one only with `--force`), never inside a client folder, never `IN` or `SRC` itself. Exit 0 written, 1 the result does not pass or a tool refuses (a trim outside the animation, another skeleton as `SRC`; the reason is printed, nothing is written), 2 a bad argument (a factor outside 0.1 to 10, also the factor that `--seconds` needs, `--start` not below `--end`, a bone that `IN` does not have, no track chosen), 3 `IN` or `SRC` is not an `.ANI` or does not read, 4 the output exists, is inside a client folder or is `IN` or `SRC`, 5 a bug (the file written did not read back and pass). With `--json`: `{"input", "tool", "out", "bytes", "written": true, "passes": true, "end_key": {"before", "after"}}`, or `{"input", "tool", "written": false, "error"}` with exit 1. Validated offline against the toolkit's own pose rules; the commands say nothing about how the game plays the result.
- `export` and `import` write only the `-o` file, as a new file; an existing one is replaced only with `--force` (written beside it as a temp file, checked, then renamed: the old file survives a failed check). `-o` is refused (exit 4) inside a client folder (a folder with `data.sah`, one of the saved client folders, or a folder below either, also through a link) and when it is the input or the `--like` file. Put the result into a client with Replace or `archive replace`.
- `export --format gltf|glb` writes one `.ANI`, `.3DC` (with `--anim ANI`, repeatable, the animations that move it), `.3DO` or `.3DE` as glTF 2.0 instead of JSON: `-o` ends in `.gltf` (a `.bin` is written beside it) or `.glb`; `--out-dir` writes `<archive path>.gltf` and `.bin` (or `.glb`). `--allow-lossy` projects a sheared, mirrored or NaN placement and drops keys that do not increase, `--pad-end` adds a key at the end key to every channel that stops earlier, `--texture-uri-base DIR` writes texture references as `DIR/<name>.png` (names only; no picture is written or embedded). **glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes.** The output rules are those of the JSON export (exit 4 for an existing output, a client folder or the input; the `.bin` counts too); exit 2 for an output name that does not fit the format or an option that needs glTF, 3 for a file that cannot be written as glTF (the reason names the bone or vertex), 5 when the toolkit's own check of what it made fails. See [Taking models and animations to other programs](../guides/interchange.md).
- `import IN.gltf|IN.glb -o OUT.ani|.3dc|.3do|.3de` reads a glTF 2.0 file as a new game file; the extension of `-o` says which. **glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes.** `--like TARGET [--from ARCHIVE]` is required for an `.ANI` (an `.ANI` file) and a `.3DC` (a `.3DC`, or the `.ANI` of its rig) and refused for a `.3DO` or `.3DE` (exit 2). The skeleton, the reference matrices and the first field of an `.ANI` come from `TARGET`; a node is a bone when it is named `Bone_NNN` (any number of digits) or listed in `--map FILE` (lines `node name = bone index`). The animation is sampled at every whole key (30 per second); `--keys exact` keeps the file's own keys (each a multiple of 1/30 second, `LINEAR` only) and `--reduce` drops keys that the others reproduce within 1e-5 (they cannot be used together); `--reduce-influences` (for a `.3DC`) keeps the two largest influences of a vertex that has more than two instead of refusing the file. An `.ANI` is written only when it passes the same check as `compare` (the lines are printed as `import` of a JSON prints them; exit 1 and `nothing written` otherwise); a `.3DC` only when its bone count is the like file's and no vertex names a bone the rig lacks. Exit 3, with the reasons naming the node, bone or vertex, for a file the importer refuses (scale on a bone, morph targets, sparse, normalized or quantized data, a required extension, a buffer that is a URL or outside the folder of the `.gltf`, a skeleton that is not the like file's, more than two influences without `--reduce-influences`, and the others listed in [the guide](../guides/interchange.md#5-bringing-a-gltf-back-in)) and for one that is truncated or malformed; exit 2 for a bad argument; exit 4 and 5 as `export`. An `.ANI` comes back with the same poses, not necessarily the same bytes; a `.3DC` the toolkit exported comes back byte for byte with that file as `--like`. See [Taking models and animations to other programs](../guides/interchange.md).
- `export --format obj` writes a `.3DO`, `.3DE` or (at rest, bones and weights left out) `.3DC` as Wavefront OBJ: `-o` ends in `.obj`, and a `.mtl` of texture names is written beside it only when the mesh names a texture (an existing `.mtl` counts like the `.bin` of a glTF: exit 4 without `--force`). `export --format bvh` writes an `.ANI` as BVH: `-o` ends in `.bvh`, one frame per whole key from the start key to the end key at 30 keys per second, the root with position channels and every joint with ZXY rotations; a non-root bone that moves relative to its parent is exit 3 unless `--drop-translation` leaves its motion out, and `--scale F` multiplies offsets and positions. Both take `-o` only (`--out-dir`, `--anim`, `--allow-lossy`, `--pad-end` and `--texture-uri-base` are exit 2, and `--drop-translation` and `--scale` are exit 2 for any other format). **glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes.** Exit codes as the glTF export. See [Taking models and animations to other programs](../guides/interchange.md).
- `import IN.obj -o OUT.3do|.3de` reads a Wavefront OBJ as a new `.3DO` or `.3DE`: triangles and quads, negative indices, one material (the texture name), missing normals computed and said, a missing texture coordinate, a second material or more than 65,535 vertices refused (exit 3, the reason names the line); the `.mtl` that `mtllib` names is never opened. `-o` of another kind, `--like` and the glTF-only options are exit 2. `import IN.bvh -o OUT.ani --like TARGET [--from ARCHIVE]` reads a BVH laid out as `export --format bvh` writes it (joints `Bone_NNN`, the channel order above, `Frame Time` 1/30, offsets equal to the like file's reference translations, only the root with position channels) as a new `.ANI` of the like file's skeleton; `--like` is required (exit 2 without it) and the `.ANI` is written only when it passes the `compare` check (exit 1 and `nothing written` otherwise). **glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes.** Output rules, exit 3, 4 and 5 as for the glTF import. See [Taking models and animations to other programs](../guides/interchange.md).
- Size caps: a model file over 128 MiB is not read by any `anim` command (`FAILED: larger than 128 MiB: not read`, exit 3); a JSON over 256 MiB is not read by `import`.

```
$ shaiya anim info --from "@us" "character/human/ani/huwm_000_*"
$ shaiya anim validate --from "@us"
$ shaiya anim compare new.ani "Character/Human/ANI/huwm_117_x.ANI" --from "@us"
$ shaiya anim export "Character/Human/ANI/huwm_117_x.ANI" --from "@us" -o wave.json
$ shaiya anim export "Character/Human/ANI/" --from "@us" --out-dir json-out
$ shaiya anim export "Character/Human/3DC/huwm_torso001.3DC" --from "@us" --format glb --anim "Character/Human/ANI/huwm_000_normal.ANI" -o torso.glb
$ shaiya anim import walk.glb -o walk.ani --like "Character/Human/ANI/huwm_000_normal.ANI" --from "@us" --keys exact
$ shaiya anim diff "Character/Human/ANI/huwm_117_x.ANI" "Character/Human/ANI/huwm_118_x.ANI" --from "@us"
$ shaiya anim edit retime "Character/Human/ANI/huwm_117_x.ANI" --from "@us" --factor 0.5 -o fast.ANI
$ shaiya anim edit trim "Character/Human/ANI/huwm_117_x.ANI" --from "@us" --start 10 --end 40 -o short.ANI
$ shaiya anim edit copy-track "Character/Human/ANI/huwm_117_x.ANI" --from "@us" --source "Character/Human/ANI/huwm_118_x.ANI" --bones 3,4,5 --rotation -o mixed.ANI
$ shaiya anim import wave.json -o new.ANI --like "Character/Human/ANI/huwm_117_x.ANI" --from "@us"
```

JSON. `info`: `{"files": [...]}` where an `.ANI` entry is `{"path", "format": "ani", "kind", "rig", "race", "bones", "end_key", "seconds", "rotation_keys", "translation_keys", "first_field", "first_field_inferred": "start key", "issues": [{"code", "severity", "bone", "message"}]}`, a `.3DC` entry is `{"path", "format": "3dc", "layout": "skinned" or "skinned-with-second-mesh" or "cloak", "vertices", "triangles", "bones", "max_vertex_bone"}`, a `.VANI` entry is `{"path", "format": "vani", "items", "vertices_per_step", "triangles", "steps", "frame_ms", "frame_ms_inferred": "milliseconds per step", "textures", "collision_mesh"}`, a `.MANI` entry is `{"path", "format": "mani", "size": 108, "fields": [{"offset", "name", "value", "raw", "meaning", "inferred": true}]}` (a float that is not finite is written as text; `raw` is the stored bytes in hex), a `.3DO` entry is `{"path", "format": "3do", "vertices", "triangles", "frames", "frame_rate", "frame_rate_inferred": "frames per second", "texture"}`, a `.3DE` entry is `{"path", "format": "3de", "layout": "standard" or "static", "vertices", "triangles", "frames", "max_key" (null for the static layout), "max_key_inferred": true, "texture"}`, and a file that does not read is `{"path", "error"}`. `validate`: the same plus `"errors"` and `"warnings"` (totals). `compare`: `{"candidate", "target", "same_skeleton", "same_first_field", "passes", "differences": [{"kind": "bone_count" or "parent" or "matrix" or "first_field", "skeleton", "bone", "candidate", "target"}] (a `first_field` entry also has `"inferred": "start key"`), "candidate_issues": [...]}` (every difference, not only the first 10). A `compare` that refuses a name with another extension prints `{"candidate", "target", "error"}` and exits 3.

JSON for `export` and `import`. `export`: `{"input", "format": "ani" or "3dc" or "vani" or "mani" or "3do" or "3de", "schema", "out", "bytes"}` (`bytes` is the length of the JSON written). `import`: `{"input", "format", "out", "bytes", "written": true}` and, for an `.ANI`, the keys of `compare` as well (`"candidate"` is `IN.json` as given and `"target"` is `--like` as given; with `--from`, the archive path). An `.ANI` that does not pass prints the keys of `compare` plus `"written": false` and exits 1.

## `shaiya svmap`

Server map files (`<id>.svmap`; the format is in [formats/svmap.md](../formats/svmap.md)) and a client's `.wld` files. Nothing here edits the game: `build` is the only subcommand that writes, and only to the `-o` file. The editor's former command line (`svmap-editor check|dump|build|roundtrip|diff|wld-check|spawns|bulk`) maps one to one onto `shaiya svmap <command>` with the same arguments, but with this page's exit codes and JSON (the editor's 0/1/2 are gone) and with `--json` as the global flag.

| Command | Arguments and flags | Exit |
|---|---|---|
| `check` | `FILE\|FOLDER` (or `--maps MAPS` instead) `--client CLIENT` `--server-data DIR` `--maps MAPS` (needs `--server-data`, not with `FILE\|FOLDER`) `--afterglow-defs PATH` (needs `--server-data`) `--target original\|afterglow` | 0 no error finding; 1 an error finding; 2 usage; 3 a map unreadable, no `.svmap`, client cannot be opened (a `--client` folder with no `data.sah` and no extracted client files counts), `--server-data` folder missing; 4 `CLIENT` is a file other than `data.sah` |
| `dump` | `FILE` | 0; 3 unreadable, not a map, a NaN or infinity with `--json` |
| `build` | `JSON -o OUT.svmap` `--force` | 0 written; 3 JSON unreadable or invalid; 4 `OUT` exists without `--force`, or `OUT` is the input; 5 the output did not read back to the same map (nothing is written) |
| `roundtrip` | `FOLDER` | 0 every file identical (binary and through JSON); 1 a difference; 3 an unreadable file, no `.svmap` |
| `diff` | `A B` (two files or two folders) | 0 identical; 1 different, or a map only on one side; 2 one file and one folder; 3 unreadable or missing input (the message names the missing path) |
| `wld-check` | `CLIENT` | 0 every `world/*.wld` writes back byte for byte; 1 a file differs; 3 a `.wld` unreadable (an unknown layout counts), no `world/*.wld`, client cannot be opened; 4 `CLIENT` is a file other than `data.sah` |
| `spawns` | `FOLDER` `--client CLIENT` | 0 report made (it reports, it does not judge); 3 an unreadable file, no `.svmap`; 4 as for `check` |
| `bulk` | `FOLDER --rule "<where> => <action>"` `--client CLIENT` `--allow-above-8` | 0 plan made; 2 the rule does not parse; 3 an unreadable file, no `.svmap`; 4 as for `check` |

- `CLIENT` (`--client` of `check`, `spawns` and `bulk`, the argument of `wld-check`) is a client folder (archive or extracted), its `data.sah`, or `@name`. Any other file is refused (exit 4), a missing path is exit 3. A folder without `data.sah` is looked into only as far as 4 folders deep and 20,000 entries; one that holds no client files within that (a drive root, say) is refused at once with exit 3 and the note "stopped looking after 20000 entries" instead of being searched to the end.
- When a command could make its result, files it could not read are reported **inside** it and raise the exit code to 3. The `{"error": ...}` object is printed only when there is no result (a missing folder, a client that cannot be opened). The exit code is the highest met. A usage error found at run time (`--server-data` with a single file, `diff` of one file and one folder, a `bulk` rule that does not parse) is exit 2, with `--json` `{"error": {"kind": "usage", "message": ...}}`.
- `check` on a folder also runs the cross-file rules (portal destinations between maps). `--client` adds the rules against the client's data (`Monster.SData`, `NpcQuest.SData`, `world/<id>.wld`); `--server-data DIR` (the server's Data folder: `Map.ini`, `MapWar.ini`, `NpcQuest.SData`, `Obelisk.ini`; a folder of maps only) adds the server and client rules; `--afterglow-defs` adds the monster check; `--target` says which server the cross-file rules judge for (default `original`). Text output lists errors and warnings per file, then totals per rule; information findings appear only in the totals.
- `build` reads the JSON that `dump --json` prints. It builds the file in memory, reads it back and compares it with the map before anything is written, then writes it atomically. An existing `OUT` stops with exit 4 unless `--force`; `OUT` equal to the input is refused even with `--force` (there is no `--in-place`; the message asks for a different path).
- `diff` of two folders compares the maps by id. Text rows show the counts of records only in A, only in B and changed per section.
- `wld-check` reads every `world/*.wld` with the lossless model, writes it back in memory and says for each file whether the bytes come back identical. That is all it checks.
- `bulk` is a dry run: it reports what the rule would do to the monster areas of every map. It never writes.
- `check --server-data GAME --maps MAPS` is for a server whose INIs and maps sit in two folders (Shaiya-GX documents `data/game` and `data/maps`; [Shaiya-GX folders](../guides/shaiya-gx.md)): `MAPS` is the folder of maps, `GAME` the folder of the INIs and `NpcQuest.SData`, and nothing is looked for in a `Map` child. Give the maps either as the first argument or with `--maps`, not both (exit 2), and `--maps` needs `--server-data` (exit 2). With `--maps` a missing `MapWar.ini` is said as `note: info: MapWar.ini not found ...` instead of the plain note; the exit code stays by the findings. Tests: `check_with_split_server_folders`, `the_maps_option_needs_server_data_and_excludes_the_target`.
- `check --server-data DIR` still exits by the findings alone when `Map.ini`, `MapWar.ini`, `NpcQuest.SData` or `Obelisk.ini` is missing from DIR, but says so: one `note:` line (and `notes` entry) per file whose rules are skipped.
- Notes (a client file that was skipped, a missing server data file, a `.svmap` left out of `spawns`, `bulk` or a folder `diff` because its file name is not a map id, an episode note such as `Monster.SData did not read with the EP4.5 layout: ...`) go to stderr as `note:` lines (silenced by `-q`) and into the `notes` array of the JSON.

```
$ shaiya svmap roundtrip maps/
77 files: binary roundtrip identical 77, JSON roundtrip identical 77, differs 0, unreadable 0

$ shaiya svmap check maps/ --client "@us"
...
errors 0 (+0 unreadable), warnings 146, info 123

$ shaiya svmap bulk maps/ --rule "mob:1 => count +1"
```

JSON (keys are snake_case; a `Finding` is `{"severity": "error"|"warning"|"info", "section": "size"|"blocked"|"section_a"|"section_b"|"mob_areas"|"npcs"|"portals"|"binds"|"regions", "index": n or null, "sub": n or null, "code", "message"}`):

- `check`: `{"target", "kind": "file"|"folder", "maps": [{"map": id or null, "file", "read_error": null or text, "findings": [Finding]}], "server_and_client": null or [{"map", "finding": Finding}], "notes": [...], "totals": {"files", "unreadable", "errors", "warnings", "info"}}` (`server_and_client` is `null` without `--server-data`).
- `dump`: the map document itself (`size`, `blocked` as base64, and one array per record section), exactly what `build` reads.
- `build`: `{"out", "bytes", "size", "blocked", "counts": {"section_a", "points_b", "mob_areas", "npcs", "portals", "binds", "regions"}}`.
- `roundtrip`: `{"folder", "files": [{"file", "map", "binary": "identical"|"differs"|"unreadable", "json": "identical"|"differs"|"failed"|"not_run", "first_difference", "length_in", "length_out", "message"}], "totals": {"files", "binary_identical", "json_identical", "differs", "unreadable"}}`.
- `diff` of files: `{"a", "b", "kind": "files", "identical", "sections": [{"section", "added", "removed", "changed"}], "entries": [{"section", "kind": "added"|"removed"|"changed", "index", "index_b", "text"}]}`. Of folders: `{"a", "b", "kind": "folders", "rows": [{"map", "status": "same"|"differs"|"only_a"|"only_b"|"unreadable", "counts", "message"}], "totals": {"same", "differs", "only_a", "only_b", "unreadable"}, "notes": [...]}`, where `counts` (only for `differs`, else `null`) is `{"sections": [{"section", "only_a", "only_b", "differ"}], "cells": n, "size": {"a", "b"} or null}` (`cells` are blocked cells that differ; `size` is set when the sizes differ).
- `wld-check`: `{"client", "files": [{"path", "bytes", "status": "identical"|"differs"|"unreadable", "first_difference", "written", "reason"}], "totals": {"files", "identical", "differs", "unreadable"}}`.
- `spawns`: `{"maps": [{"map", "areas", "monsters", "kinds", "range": [lo, hi] or null, "findings": {"<group>": n}}], "totals": {"maps", "areas", "monsters", "kinds", "findings": {...}, "never_spawned": n or null}, "findings": [{"map", "group", "areas": [...], "monsters": [...], "text"}], "never_spawned": [ids] or null, "unreadable": [...], "notes": [...]}` (`never_spawned` needs `--client` with a readable `Monster.SData`).
- `bulk`: `{"rule", "dry_run": true, "maps": [{"map", "matched", "changed", "entries", "monsters_before", "monsters_after", "held_at_8", "emptied", "deleted"}], "totals": {"matched", "maps_matched", "changed", "maps_changed", "entries", "held_at_8", "emptied"}, "unreadable": [...], "notes": [...]}`.

The full user guide and the format pages come with the editor's documentation.

## `shaiya loose`

Changes to a loose data folder (a client without `data.sah`; [the folder rules](../formats/loose-client.md)). `CLIENT` is `@name` of a saved loose client that has a confirmed backup folder, or a folder that passes the detection together with `--backups DIR` (outside the data folder; a saved client that has none can be given one this way for one run). Every write needs the backup folder: a missing one is exit 4 ("choose a backup folder outside the data folder") and nothing is written anywhere (test `loose_replace_then_rollback_restores_the_bytes`). An archive client or any other folder is exit 2 (test `loose_commands_refuse_an_archive_client`).

| Command | What it does | Exit |
|---|---|---|
| `list CLIENT [PATTERN...]` | The files below the data folder (letter case ignored in the patterns), with sizes | 0; 2; 3 |
| `add CLIENT FILE... [--at DIR \| --as PATH]` | Puts new files into the data folder: under `--at DIR` (default: the top) with their own names, or one file at `--as PATH`. A file that is already there is refused ("use shaiya loose replace") | 0; 2; 3; 4 |
| `replace CLIENT FILE... [--at DIR \| --as PATH]` | The same for files that are there; one that is not is refused ("use shaiya loose add") | 0; 2; 3; 4 |
| `remove CLIENT PATH...` | Removes files; a copy goes to the journal | 0; 2; 3; 4 |
| `apply CLIENT PATCH [--key PUBLIC.key or --public-hex HEX with --check-version N]` | Applies a patch as one change: its archive entries and loose files become puts by path, its `remove.txt` paths become removes (a path that is not there is left out and said; one that names converted Shaiya-GX media on a Shaiya-GX layout refuses the patch). A loose file of the patch must go into a folder the data folder has. With a key the patch's signature must verify, before anything is read and again right before the write (without a key nothing is checked, as for `shaiya patch apply`; use `shaiya patch verify` first). Journal kind `loose-patch` | 0; 2; 3; 4 |
| `history CLIENT` | The journals of the backup folder, each with whether it can be rolled back | 0; 2 |
| `rollback CLIENT [--id ID] [--force]` (alias `undo`) | Undoes the newest applied change (or `--id`: the whole id or an ending that names exactly one journal); `--force` saves and then restores files that were changed since | 0; 2; 4 |
| `prune CLIENT --keep N` | Deletes the oldest journals beyond the newest N of this client (never one that did not finish; refused, exit 4, while a change is running or an earlier change or rollback did not finish) | 0; 2; 4 |
| `health CLIENT` | Reads the folder and writes nothing: unreadable files, model files that do not read to their last byte and SData files whose SEED wrapper fails (the table inside is not read), model textures found nowhere, temporary files, journals (damaged journal folders are named), an unfinished change, a change that is running now | 0 nothing found; 1 something found; 2; 3 |
| `recover CLIENT` | Undoes an interrupted change or finishes an interrupted rollback; refused (exit 4, "another change to this folder is running now") while the change runs | 0; 2; 4 |

`add`, `replace`, `remove` and `apply` take `--dry-run` (show the checked change and stop; nothing is written, not even the backup folder). They print the confirmation lines on stderr, as the archive commands do: `Changes`, `Writes`, `Undo`, `Game` ("restart the game to see the change"). **The game question.** On Windows the toolkit refuses while `Shaiya GX.exe` or `game.exe` in the install folder is open in another program. When it cannot tell, a write asks "the toolkit cannot tell whether the game is running: stop it first" on a terminal and is exit 4 without one, unless `--yes-game-stopped` says it is stopped (tests `an_unknown_busy_state_needs_the_confirmation`, `loose_replace_then_rollback_restores_the_bytes`). `--json` prints the report of a write (`journal`, `added`, `replaced`, `removed`, `recovered`) or the plan of a dry run.

## `shaiya clients`

The saved client folders live in the toolkit's data folder (`shared.json`), shared by the CLI and Shaiya Toolkit. The toolkit never scans your disks for clients.

| Command | What it does | Exit |
|---|---|---|
| `list` | Table of name, status (`found` or `missing`), path, note. JSON: `{"clients": [{"name", "path", "note", "added_utc", "exists"}]}` | 0 |
| `add NAME PATH [--note TEXT]` | Saves a folder (stored as an absolute path; it must hold `data.sah`). `NAME` is 1 to 40 characters without `/`, `\` or `@` | 0; 3 no `data.sah`; 4 bad or duplicate name |
| `remove NAME` | Forgets the name; no file is touched | 0; 3 unknown name |
| `show NAME` | Path, SAH version, file count, `Version.ini` CheckVersion and CurrentVersion, whether `game.exe` is there, number of journals. JSON: `{"name", "path", "note", "added_utc", "sah_version", "files", "check_version", "current_version", "game_exe", "journals"}` | 0; 3 |
| `health NAME\|DIR` | Looks at a saved client (`NAME` or `@name`) or a client folder and writes nothing: the index check, `data.saf` size and dead space, journals, a pending change, `Version.ini` `StartUpdate=UPDATE_START`, the leftovers and the earlier cleanup folders. [What it reports](../guides/client-health.md). | 0 nothing found; 1 something found; 3 the index cannot be read, unknown name |
| `clean NAME\|DIR --move KIND... [--dry-run]` | Moves leftovers into `shaiya-toolkit-backups/cleanup-<UTC>/` of the client folder and deletes none of them. `KIND` is `sah-bak`, `stray-temp`, `swap-pair`, `editor-backup` or `editor-first-copy`; repeat `--move` or separate with commas. It first finishes or discards an interrupted change, by the same recovery every Toolkit write runs. `--dry-run` lists what would be moved and changes nothing. | 0; 2 no `--move` or an unknown kind; 3 a file error, including the manifest failing to be rewritten after a move (the message names the cleanup folder to Put back); 4 the game or another program holds the files, a pending change is still recorded after recovery, an editor kind is asked for while `svmap-editor-install.json` is in the folder, or a rename fails partway through the clean-up (the message says how many files were moved and names the cleanup folder; Put back there restores them) |
| `clean --put-back DIR` | Moves every file a cleanup folder's `manifest.json` lists back to its old name. | 0; 3 some file is in neither place, or a rename or the manifest write fails partway (files already put back stay back; run it again); 4 a destination exists or lies below a symlink or junction (nothing is moved), the manifest names an unsafe path (nothing is moved), or `DIR` is not a cleanup folder. It does not run recovery or check whether the game holds the files |

`health` with `--json` prints the report: `client`, `healthy`, `findings`, `index` (`sah_version`, `files`, `saf_present`, `problems`, `warnings`), `dead_space` (`saf_bytes`, `live_bytes`, `dead_bytes`, or `null` without `data.saf`), `journals` (`count`, `bytes`), `pending_marker` (`op`, `label`, `readable`, or `null`), `start_update`, `leftovers` (each `kind`, `id`, `files`, `bytes`, `pre_selected`, `note`), `not_offered` (`file`, `why`) and `cleanups` (`name`, `state`, `items`, `to_put_back`, `bytes`). A finding is a problem of the index check, a missing `data.saf`, a pending change, `UPDATE_START` or an offered leftover; dead space, journals and cleanup folders are not findings. `clean --json` prints `folder`, `path`, `moved`, `bytes` and `notes` (or `dry_run`, `would_move`, `files`, `bytes`; or `put_back` with `restored`, `already_in_place` and `missing`). A cleanup folder is not a journal: `patch history` does not list it. The in-use check is the one of the other writes (Windows; elsewhere a warning).

Any `ARCHIVE` or `CLIENT` argument may then be `@name` (names are matched case-insensitively).

## `shaiya version`

```
shaiya 0.5.0 (git 0123456789ab, x86_64-pc-windows-msvc)
```

With `--json`: `{"version": "0.5.0", "git": "0123456789ab", "target": "x86_64-pc-windows-msvc"}`. `shaiya -V` prints the same line.
