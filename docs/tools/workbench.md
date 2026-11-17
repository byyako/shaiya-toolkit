# Shaiya Workbench

The desktop program `shaiya-workbench`: browse and edit a client's `data.sah`/`data.saf`, build, sign and apply patches, convert SData files, and inspect files. It does the same work as the [`shaiya` command line](shaiya.md), through the same library code, with the same journals and backups. New here? Follow [the five-minute guide](../getting-started.md).

```
shaiya-workbench [FILES...] [--data-dir DIR] [--no-settings] [--smoke [REPORT_FILE]]
```

`--data-dir` keeps settings and the saved client folders in `DIR`; `--no-settings` writes no settings; `--smoke` opens the real window, renders five frames, writes `ok <adapter> <backend>` (or the error) to the report file and exits with 0 or 1, a quick check that the graphics stack works. Files named on the command line are handled like files dropped on the window. Only one Workbench should change a client folder at a time, and not while the game is running from it.

## Client folders

The toolbar picker lists your saved client folders (the same list as `shaiya clients list`, so a client added in one program shows up in the other) and remembers the last one used.

- **File > Add client folder...** (also on the picker) asks for a folder holding `data.sah` and a name. You can also drop a folder on the window and choose "Open this client".
- **File > Manage client folders...** shows each client with its status, SAH version, file count and `Version.ini` CheckVersion and CurrentVersion; rename, edit the note, remove from the list (files untouched), or **Use another archive pair...** which swaps a different `data.sah`/`data.saf` in (the current pair is kept as `data.<stamp>.sah/saf`; there is a "swap back").
- **Open other archive...** opens any `.sah` (for example a stock `data.orig.sah`) read-only; changes to it can only be saved as a new archive.
- Workbench never scans your disks for clients. When a client opens, it finishes or discards the leftovers of an interrupted change and says so in the Activity panel.

## The six tabs

**Archive.** The folder tree on the left, the file table in the middle, a preview on the right.

- Search (Ctrl+F): plain text matches part of a path; text with `*`, `?` or `[` is a [glob pattern](shaiya.md#conventions-every-command-follows-them). Filters for file type and size; a toggle shows shadowed entries.
- Preview: images (DDS, TGA, BMP) with a checkerboard, text with a choice of encoding (code page 949 first), SData with its checksum state and a "Show plaintext" switch, anything else as hex (Ctrl+G jumps to an offset). Files with no `data.saf` next to the index open in an index-only mode: no previews, extraction or changes.
- Right-click a file: Extract..., Extract with folders..., Copy path, Copy as JSON, Show in folder tree, Replace with file..., Remove, Decrypt to file... (SData).
- **Extract** asks for a folder; if files exist you choose Skip existing, Overwrite or Cancel. Progress shows in Activity and can be cancelled (files already written stay).
- **Pending changes.** Dropping files on the tab opens an "Add to archive" dialog (target folder, whether to keep the dropped folder's name, "new" or "replaces" per file); "Replace with file..." and Delete queue the other changes. Queued changes appear in a strip at the bottom and in the table (`~` replaced, struck through removed, italic added). **Nothing is written until you apply.** Ctrl+Z / Ctrl+Y undo and redo in the queue.
  - **Apply to client...** asks for confirmation, then appends to `data.saf`, writes a journal and replaces the index. The handle Workbench holds on `data.saf` is released first.
  - **Save as new archive...** writes a new, compacted pair and leaves the client alone.
  - **Make patch...** fills the Patch builder from the queue.
  - **Discard** drops the queue (asks first). Quitting with a non-empty queue asks too; "Save all" never writes a client by itself.
- **Check** (and **Deep check**) reports problems, warnings and info grouped by code, with clickable paths; Copy report and Save report. **Compare...** diffs against another archive (added, removed, different, unknown) and can "Make patch from differences". **Compact...** rewrites `data.saf` without unused space; the confirmation says how many earlier changes can then no longer be rolled back.

![The Archive tab: folder tree, file table and preview](../assets/screenshots/workbench-archive.png)

**Patch builder.** Three sources, or the differences of two archives, or the pending changes: a folder of archive files (paths inside it become archive paths), a folder of loose files (copied into the client folder, for example `game.exe`), and a removal list. The contents table shows what each entry does to the selected client (new, replaces, not present, overwrites) and a problems column; Build is disabled while a name is invalid. Output name (suggested as the next `psNNNN.patch` from the client's `Version.ini`), timestamp, optional signing with a secret key file and CheckVersion. Warnings match the CLI (the original updater only fetches `psNNNN.patch`; `remove.txt` needs the toolkit or a replacement updater). After a build: Show folder, Apply to a client..., Verify.

![The Patch builder: sources, contents table and output name](../assets/screenshots/workbench-patch-builder.png)

**Apply & history.** Add patches (drop or Add...; drag to reorder), see for each its structure check and signature state, set a public key and CheckVersion (stored per client; "Require a valid signature" turns on with a key), read the plan (added, replaced, removed, missing, loose files, bytes to append), and **Apply N patches...**. The lower half is the history of journals for this client with the rollback state and the reason when blocked; **Roll back...** undoes the newest possible one; **Prune...** keeps only the newest N.

![Apply & history: patch list, plan and the journal history with Roll back](../assets/screenshots/workbench-apply-history.png)

**Sign & verify.** Generate a key pair (secret first; existing files are refused, never overwritten), sign patches or an updater executable (`.sig` next to each file; an existing one asks "Replace signature"), and verify signatures against a public key, optionally against a client (is the patch applied?). Workbench never stores key contents; the secret key's *path* is remembered only if you tick "Remember where my secret key is".

![Sign & verify: key pair, signing and verification results](../assets/screenshots/workbench-sign-verify.png)

**SData.** Drop SData files or folders, or add all SData of the open client. The table shows encrypted or plain, checksum state and plaintext size. Decrypt to folder..., Encrypt to folder... (with "Match the originals' headers", which is what makes an unchanged file come out byte-identical) and Check. Outputs always go to a folder you choose; in-place conversion is a CLI feature.

![The SData tab: files with encryption and checksum state](../assets/screenshots/workbench-sdata.png)

**Inspect.** The file inspector: open or drop files, see what each is (SAH index, SAF data, SData encrypted or plain, patch, signature, `game.exe` build) with Name, Kind, Size and Details, the same text `shaiya info` prints. SHA-256 per row or for the selection runs in the background with progress in Activity and can be cancelled. Copy as text / Copy as JSON put the selected rows on the clipboard, handy for bug reports (never attach game files to a report; hashes are enough).

![The Inspect tab: file kinds, sizes and details](../assets/screenshots/workbench-inspect.png)

## Confirmations and prompts

Buttons say what they do and the risky one is marked.

| When | Text and buttons |
|---|---|
| Apply pending changes | "Apply N changes to NAME (folder)? X will be appended to data.saf. A backup of the index (and of files it overwrites) goes to shaiya-toolkit-backups\...; Apply & history can roll it back. The game must be closed." **Apply N changes** / Cancel. On Linux and macOS the last sentence says the game cannot be checked. |
| Apply patches | Counts, "Signatures: valid" or "NOT CHECKED", where backups go. **Apply N patches** / Cancel. An invalid signature with "Require" on disables the button and says why. |
| Compact | "...rewrites X (needs about Y free)... N earlier changes can no longer be rolled back afterwards, and compacting makes no journal, so it cannot be undone itself." **Compact** / Cancel. |
| Roll back | What will be restored (index, `data.saf` length, loose files). **Roll back** / Cancel. |
| Swap archives | "Put PAIR in place of NAME's data.sah/data.saf? The current pair is kept as data.<stamp>.sah/saf." **Swap archives** / Cancel. |
| Existing outputs | Skip existing / Overwrite / Cancel (extract, SData); Overwrite / Cancel (patch); Replace signature / Cancel. |
| Discard, reload with pending changes | Discard / Keep; reload asks first. |
| **The game or another program has the files** | "NAME's data.saf is open in another program (is the game running?). Close it and press Retry." **Retry** / Cancel. Nothing has been changed; close the program and press Retry. (Windows only; elsewhere the toolkit cannot tell.) |

## Keyboard

| Keys | Action |
|---|---|
| Ctrl+1 ... Ctrl+6 | Switch tab: Archive, Patch builder, Apply & history, Sign & verify, SData, Inspect |
| Ctrl+F | Focus the search box (Esc clears it) |
| Up, Down, PageUp, PageDown, Home, End, Shift or Ctrl+click, Ctrl+A | Move and select in the table |
| Enter | Open the selected folder; on a file, focus the preview |
| Backspace, Alt+Up | Parent folder |
| Delete | Queue removal of the selected files |
| Insert | Add files to the folder shown |
| Ctrl+E, Ctrl+Shift+E | Extract the selection, extract everything shown |
| Ctrl+Enter | Apply pending changes (Archive tab) |
| Ctrl+G | Go to an offset (hex preview) |
| Ctrl+C | Copy selected paths |
| Ctrl+Z, Ctrl+Y | Undo, redo in the pending queue |
| F5 | Reload the archive (asks first when changes are pending) |
| F1 | Help |
| Ctrl+Shift+P | Command palette: every action with its shortcut |
| Ctrl+O, Ctrl+Shift+O | Open files, open by path |
| Ctrl+J | Activity panel (progress, cancel) |
| Ctrl+, | Preferences |

(On macOS read Ctrl as Cmd.)

## The shell

Menus: File, Edit, View, Tools, Help. **Edit > Preferences** (Ctrl+,): theme (dark, light, system), UI scale, autosave and backup counts, the font used for Korean and Chinese names, whether extracted files keep the archive's pack time, and the Inspect column widths. **Help > About** shows the version, the git revision and the third-party notices; **Help > Open log file** and **Open data folder** show the toolkit's own files in your file manager.

**Help > Support the project** opens the author's donation page in your default browser. It happens only when you click it; Workbench makes no network connection of its own, and nothing is sent.

Korean and Chinese names need a font: Workbench looks for a system font at start (Malgun Gothic or Microsoft YaHei on Windows, Apple SD Gothic Neo or PingFang on macOS, Noto Sans CJK or a similar font on Linux). Name a font file in Preferences, or set `SHAIYA_TOOLKIT_FONT`, to use another one.

Settings and logs: see [install.md](../install.md#portable-mode-and-where-settings-live). Journals and backups are kept in each client folder, see [undo and recovery](../guides/undo-and-recovery.md).

## Left out on purpose

Dragging files out to the desktop, 3D model and animation previews, audio playback, text editing, SData record views (that is the planned SData Editor), several archives open at once (use Compare), in-place SData conversion, autosave of the pending queue (nothing is written before Apply, so a crash loses only the queue), and anything that uses the network.
