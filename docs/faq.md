# FAQ

## It says "data.saf is open in another program" (exit code 4)

The full message is `error: refused: data.saf is open in another program; close the game (and any other program using this client, Shaiya Toolkit included) and try again`. Windows reports that something has `data.sah` or `data.saf` open for writing: the game, the original updater, a second copy of Shaiya Toolkit, an antivirus scan, or a file manager preview. Nothing was changed. Close them and run the command again (in Shaiya Toolkit, press **Retry** in the dialog). `data.sah is read-only or not writable` means the file's read-only flag or folder permissions; clear them. On Linux and macOS the toolkit cannot tell whether the game is running and prints a warning instead, so make sure it is closed.

## Windows says "Windows protected your PC" (SmartScreen)

The programs are not code signed. Choose **More info**, then **Run anyway**. Check the download against `SHA256SUMS.txt` first ([install.md](install.md#check-the-download)). `shaiya.exe` is a command-line program. If you double-click it, it prints its help, says to open a terminal and waits for Enter before closing; open a terminal and run `shaiya --help` instead.

## macOS says the developer is unidentified

Right-click the app and choose Open, or run `xattr -dr com.apple.quarantine "Shaiya Toolkit.app"`. The macOS GUI is untested until a tester confirms it.

## My client is "unsupported" or a command fails on it

- **`unsupported version`** (exit 3): the `data.sah` has a version the toolkit does not know. Versions 0 and 1 work. Please report the client (region, episode) and the output of `shaiya info data.sah` without attaching game files.
- Archives, patches and SData wrappers work on every client in the [version matrix](version-matrix.md). What does not work everywhere is reading the *tables inside* SData files: the Monster and NpcQuest readers in `shaiya-clientdata` only understand the layouts of some clients, and the table says `fails` where the layout is newer. The SData commands still decrypt and re-encrypt those files.
- A `.sah` without its `.saf` can be listed, counted and checked (index only), but not extracted, previewed or changed.
- `error: the following required arguments were not provided: <ARCHIVE>` right after you typed `@name` in PowerShell: PowerShell treats a bare `@name` as splatting and passes nothing. Write the name in double quotes, `"@name"` (this works in `cmd.exe` too; single quotes work in PowerShell only).
- `error: ... no data.sah in this folder`: `@name` or the path does not point at the client folder; `shaiya clients list` shows what is saved.

## What do the exit codes mean?

| Code | Meaning |
|---|---|
| 0 | Success; checks passed; no differences |
| 1 | The answer is "no": a check, a CRC or a verification failed, or archives differ |
| 2 | Wrong usage (unknown flag, missing argument) |
| 3 | Input problem: missing or unreadable file, unknown `@name`, bad format, checksum failure |
| 4 | Refused by a safety rule: output exists, neither `--in-place` nor `-o`, game running, rollback blocked, bad names |
| 5 | Internal error: please report it |
| 130 | Cancelled |

Details per command: [tools/shaiya.md](tools/shaiya.md#exit-codes).

## It stopped with "refused: this would change a file"

Commands that change an archive need to know where to write: `--in-place` for the client's own files (with a journal) or `-o OUT` for a new archive pair. Without either the toolkit refuses (exit 4) and changes nothing. `--dry-run` shows what would happen first.

## How do I undo a change?

`shaiya patch history "@name"`, then `shaiya patch rollback "@name"`. In Shaiya Toolkit: Apply & history > Roll back.... What blocks it (a newer change, a compaction, a swap, `--no-journal`) and how interrupted changes are recovered: [undo and recovery](guides/undo-and-recovery.md).

## Where are the backups?

Journals are in `<client folder>/shaiya-toolkit-backups/`, one folder per change. `data.sah.bak` next to the index is a plain copy of the index from before the *latest* change and is overwritten by the next one. Changes made with `--no-journal` have no journal, and loose files they overwrite have no backup. `compact --in-place` cannot be undone. Details: [undo and recovery](guides/undo-and-recovery.md#what-is-kept-and-where).

## The game does not start, or shows wrong data, after a change

Roll the change back (`shaiya patch rollback "@name"`) and check the client with `shaiya archive check "@name" --deep`. Typical causes: an SData file re-encrypted without `--like` (see [SData files](guides/sdata-files.md)), a replaced file in the wrong format, or a patch applied to a different client generation than it was made for.

## `data.saf` keeps growing

Every change appends to `data.saf` and leaves the old bytes behind. `shaiya archive stats "@name"` shows the unused space; `shaiya archive compact "@name" --in-place` reclaims it, but it cannot be undone and ends rollback for earlier changes. Use `compact -o NEWDIR` for a compacted copy.

## "signature not checked" when applying a patch

You did not give a public key. To accept only patches you signed, pass `--key public.key --check-version N` ([make a patch](guides/make-a-patch.md#4-sign-it-recommended)).

## Does it work with the original Updater.exe?

Patches use the same `psNNNN.patch` layout. The original updater ignores `remove.txt`, so a patch that relies on removals only works through this toolkit or a replacement updater.

## Which episodes does the SVMap Editor support?

It was written for and verified on the US EP4.5 client (patch 108), the original EP4.5 server files and the Afterglow server. There is no detection of the client version. Every file is checked by whether it reads: a map file that does not read is refused, and a client table that does not read turns off the checks that need it, with a note on the client chip. The names of the map-name table are the EP4.5 client's, so on another client the info rule map-no-name may be wrong. The table of assumptions is in [the editor guide](tools/svmap-editor.md#verified-on-and-other-episodes) and the [version matrix](version-matrix.md#svmap-editor-and-shaiya-svmap).

## Why does a map open read-only on the client side?

The editor only writes a client map (`world/<id>.wld`) when the file reads and writes back byte for byte. The game reads a map file with no length check, so a wrong count would make it read garbage. A file that does not write back identically stays read-only on the client side: edits change the server map only, and the status bar says "client map read-only". `shaiya svmap wld-check <client folder>` names the first byte that differs. See [the World format page](formats/wld.md).

## Why is the exported patch unsigned?

The original updater does not check signatures, and the editor holds no key. An updater that requires signed patches needs a signature file: sign the patch with `shaiya patch sign` or in Shaiya Toolkit (Sign & verify) ([make a patch](guides/make-a-patch.md#4-sign-it-recommended)).

## Can the editor change game.exe or a DLL?

No. The editor and `shaiya svmap` change data only. A staged file whose name ends in `.exe` or `.dll` is refused by Export patch, Export folder, Install and Copy loose. This is why a new map's name cannot be added by the editor: the table of map names is in the game's program. The editor writes a note file with the name you chose instead.

## Where are the editor's settings and backups?

In the `shaiya-svmap-editor` folder inside the toolkit data folder ([install.md](install.md#portable-mode-and-where-settings-live)); **Help > Open data folder** in the editor opens it. It holds `settings.json`, `log.txt`, `recovery/` (unsaved work after a crash) and `backups/` (a copy of every map file before a save). The first version of a saved map is also kept as `<id>.svmap.bak` beside it. Install into a test client keeps its own index copies in that client folder.

## Does the toolkit send anything over the network?

No. No program or library contains network code and nothing is downloaded. Shaiya Toolkit's **Help > Support the project** item opens a web page in your own browser, only when you click it.

## Can I attach my game files to a bug report?

No. Attach the output of `shaiya info` or `shaiya version`, the command you ran, the error text, and a SHA-256 (`shaiya info --hash FILE`) if you need to identify a file. Report problems on the [issues page](https://github.com/byyako/shaiya-toolkit/issues); security problems privately, see [SECURITY.md](../SECURITY.md).
