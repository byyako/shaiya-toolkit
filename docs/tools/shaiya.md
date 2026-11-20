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
- **`--json`**: stdout holds exactly one JSON value, the result object or an error object, for example `{"error": {"kind": "format", "message": "Item.SData: SEED checksum failed (at byte 40)", "file": "Item.SData", "offset": 40}}`. The error kinds are `io`, `format`, `unsupported`, `refused`, `cancelled`, plus `usage` (exit 2) and `internal` (exit 5) from `shaiya svmap`. Keys are snake_case, sizes are integers in bytes, hashes lower-case hex, times ISO-8601 UTC, archive paths use `/`. Adding a key is not a breaking change; renaming or removing one is (CHANGELOG "Breaking").
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

Only the file name's extension decides which files are looked at (`.SData`, `.dds`, `.tga` in any letter case). The checks read the file once and do not parse SData tables.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success; checks passed; no differences. |
| 1 | The answer is "no": differences found, a check, a CRC or a verification failed. |
| 2 | Usage error (unknown flag, missing argument). |
| 3 | Input error: unreadable or missing file, unknown `@name`, bad format, checksum failure, unsupported version. |
| 4 | Refused by a safety rule: existing output, no `--in-place` or `-o`, the game or another program has the files open, blocked rollback, bad names. |
| 5 | Internal error: a panic, or the one detected bug of `svmap build` (the output did not read back to the same map). The message asks for a bug report. With `--json` the second case prints `{"error": {"kind": "internal", "message": ...}}`. |
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

Kinds: SAH index, SAF data (with the file count and live bytes from a `.sah` next to it), SData (SEED-encrypted, checksum checked; or plain), patch (archive files, loose files, removals, whether a `.sig` sits next to it), patch signature, Windows program (machine and link time), unknown (the first 16 bytes). A damaged file is reported with a `problem:` line, not a failure; files over 512 MiB are not decrypted for the checksum check.

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

## `shaiya svmap`

Server map files (`<id>.svmap`; the format is in [formats/svmap.md](../formats/svmap.md)) and a client's `.wld` files. Nothing here edits the game: `build` is the only subcommand that writes, and only to the `-o` file. The editor's former command line (`svmap-editor check|dump|build|roundtrip|diff|wld-check|spawns|bulk`) maps one to one onto `shaiya svmap <command>` with the same arguments, but with this page's exit codes and JSON (the editor's 0/1/2 are gone) and with `--json` as the global flag.

| Command | Arguments and flags | Exit |
|---|---|---|
| `check` | `FILE\|FOLDER` `--client CLIENT` `--server-data DIR` `--afterglow-defs PATH` (needs `--server-data`) `--target original\|afterglow` | 0 no error finding; 1 an error finding; 2 usage; 3 a map unreadable, no `.svmap`, client cannot be opened (a `--client` folder with no `data.sah` and no extracted client files counts), `--server-data` folder missing; 4 `CLIENT` is a file other than `data.sah` |
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

## `shaiya clients`

The saved client folders live in the toolkit's data folder (`shared.json`), shared by the CLI and Shaiya Toolkit. The toolkit never scans your disks for clients.

| Command | What it does | Exit |
|---|---|---|
| `list` | Table of name, status (`found` or `missing`), path, note. JSON: `{"clients": [{"name", "path", "note", "added_utc", "exists"}]}` | 0 |
| `add NAME PATH [--note TEXT]` | Saves a folder (stored as an absolute path; it must hold `data.sah`). `NAME` is 1 to 40 characters without `/`, `\` or `@` | 0; 3 no `data.sah`; 4 bad or duplicate name |
| `remove NAME` | Forgets the name; no file is touched | 0; 3 unknown name |
| `show NAME` | Path, SAH version, file count, `Version.ini` CheckVersion and CurrentVersion, whether `game.exe` is there, number of journals. JSON: `{"name", "path", "note", "added_utc", "sah_version", "files", "check_version", "current_version", "game_exe", "journals"}` | 0; 3 |
| `health NAME\|DIR` | Looks at a saved client (`NAME` or `@name`) or a client folder and writes nothing: the index check, `data.saf` size and dead space, journals, a pending change, `Version.ini` `StartUpdate=UPDATE_START`, the leftovers and the earlier cleanup folders. [What it reports](../guides/client-health.md). | 0 nothing found; 1 something found; 3 the index cannot be read, unknown name |
| `clean NAME\|DIR --move KIND... [--dry-run]` | Moves leftovers into `shaiya-toolkit-backups/cleanup-<UTC>/` of the client folder and deletes nothing. `KIND` is `sah-bak`, `stray-temp`, `swap-pair`, `editor-backup` or `editor-first-copy`; repeat `--move` or separate with commas. It finishes an interrupted change first. `--dry-run` lists what would be moved and changes nothing. | 0; 2 no `--move` or an unknown kind; 3; 4 the game or another program holds the files, a pending change is still recorded after recovery, or an editor kind is asked for while `svmap-editor-install.json` is in the folder |
| `clean --put-back DIR` | Moves every file a cleanup folder's `manifest.json` lists back to its old name. | 0; 3 some file is in neither place; 4 a destination exists (nothing is moved), or `DIR` is not a cleanup folder |

`health` with `--json` prints the report: `client`, `healthy`, `findings`, `index` (`sah_version`, `files`, `saf_present`, `problems`, `warnings`), `dead_space` (`saf_bytes`, `live_bytes`, `dead_bytes`, or `null` without `data.saf`), `journals` (`count`, `bytes`), `pending_marker` (`op`, `label`, `readable`, or `null`), `start_update`, `leftovers` (each `kind`, `id`, `files`, `bytes`, `pre_selected`, `note`), `not_offered` (`file`, `why`) and `cleanups` (`name`, `state`, `items`, `to_put_back`, `bytes`). A finding is a problem of the index check, a missing `data.saf`, a pending change, `UPDATE_START` or an offered leftover; dead space, journals and cleanup folders are not findings. `clean --json` prints `folder`, `path`, `moved`, `bytes` and `notes` (or `dry_run`, `would_move`, `files`, `bytes`; or `put_back` with `restored`, `already_in_place` and `missing`). A cleanup folder is not a journal: `patch history` does not list it. The in-use check is the one of the other writes (Windows; elsewhere a warning).

Any `ARCHIVE` or `CLIENT` argument may then be `@name` (names are matched case-insensitively).

## `shaiya version`

```
shaiya 0.3.1 (git 0123456789ab, x86_64-pc-windows-msvc)
```

With `--json`: `{"version": "0.3.1", "git": "0123456789ab", "target": "x86_64-pc-windows-msvc"}`. `shaiya -V` prints the same line.
