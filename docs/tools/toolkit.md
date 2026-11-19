# Shaiya Toolkit

Shaiya Toolkit is the desktop app (`shaiya-toolkit`): browse and edit a client's `data.sah`/`data.saf`, build, sign and apply patches, convert SData files, and inspect files. It does the same work as the [`shaiya` command line](shaiya.md), through the same library code, with the same journals and backups. New here? Follow [the five-minute guide](../getting-started.md).

```
shaiya-toolkit [FILES...] [--data-dir DIR] [--no-settings] [--smoke [REPORT_FILE]]
```

`--data-dir` keeps settings and the saved client folders in `DIR`; `--no-settings` writes no settings; `--smoke` opens the real window, renders five frames, writes `ok <adapter> <backend>` (or the error) to the report file and exits with 0 or 1, a quick check that the graphics stack works. Files named on the command line are handled like files dropped on the window. Only one copy of Shaiya Toolkit should change a client folder at a time, and not while the game is running from it.

## Client folders

The toolbar picker lists your saved client folders (the same list as `shaiya clients list`, so a client added in one program shows up in the other) and remembers the last one used.

- **File > Add client folder...** (also on the picker) asks for a folder holding `data.sah` and a name. You can also drop a folder on the window and choose "Open this client".
- **File > Manage client folders...** shows each client with its status, SAH version, file count and `Version.ini` CheckVersion and CurrentVersion; rename, edit the note, remove from the list (files untouched), or **Use another archive pair...** which swaps a different `data.sah`/`data.saf` in (the current pair is kept as `data.<UTC stamp>.sah/saf`; there is a "swap back").
- **Open other archive...** opens any `.sah` (for example a stock `data.orig.sah`) read-only; changes to it can only be saved as a new archive.
- Shaiya Toolkit never scans your disks for clients. When a client opens, it finishes or discards the leftovers of an interrupted change and says so in the Activity panel.

## The Start page

With no client or archive open, the Archive tab shows a **Start** page: one line for each job and a button that opens the right place. **Add client folder...** asks for a folder and a name; **Open other archive...** opens any `data.sah` read-only; **Open Patch builder** and **Open Apply & history** switch to those tabs. The page goes away as soon as an archive opens.

## The six tabs

**Archive.** The folder tree on the left, the file table in the middle, a preview on the right.

- Search (Ctrl+F): plain text matches part of a path; text with `*`, `?` or `[` is a [glob pattern](shaiya.md#conventions-every-command-follows-them). Filters for file type and size; a toggle shows shadowed entries.
- Preview: images (DDS, TGA, BMP) with a checkerboard, text with a choice of encoding (code page 949 first), SData with its checksum state and a "Show plaintext" switch, anything else as hex (Ctrl+G jumps to an offset). Files with no `data.saf` next to the index open in an index-only mode: no previews, extraction or changes.
- Right-click a file: Extract..., Extract with folders..., Copy path, Copy as JSON, Show in folder tree, Replace with file..., Remove, Decrypt to file... (SData).
- **Extract** asks for a folder; if files exist you choose Skip existing, Overwrite or Cancel. Progress shows in Activity and can be cancelled (files already written stay).
- **Pending changes.** Dropping files on the tab opens an "Add to archive" dialog (target folder, whether to keep the dropped folder's name, "new" or "replaces" per file); "Replace with file..." and Delete queue the other changes. Queued changes appear in a strip at the bottom and in the table (`~` replaced, struck through removed, italic added). **Nothing is written until you apply.** Ctrl+Z / Ctrl+Y undo and redo in the queue.
  - **Apply to client...** asks for confirmation, then appends to `data.saf`, writes a journal and replaces the index. The handle Shaiya Toolkit holds on `data.saf` is released first.
  - **Save as new archive...** writes a new, compacted pair and leaves the client alone.
  - **Make patch...** fills the Patch builder from the queue.
  - **Discard** drops the queue (asks first). Quitting with a non-empty queue asks too; "Save all" never writes a client by itself.
- **Check** (and **Deep check**) reports problems, warnings and info grouped by code, with clickable paths; Copy report and Save report. **Compare...** diffs against another archive (added, removed, different, unknown) and can "Make patch from differences". **Compact...** rewrites `data.saf` without unused space; the confirmation says how many earlier changes can then no longer be rolled back.

**Patch builder.** Three sources, or the differences of two archives, or the pending changes: a folder of archive files (paths inside it become archive paths), a folder of loose files (copied into the client folder, for example `game.exe`), and a removal list. The contents table shows what each entry does to the selected client (new, replaces, not present, overwrites) and a problems column; Build is disabled while a name is invalid. Output name (suggested as the next `psNNNN.patch` from the client's `Version.ini`), timestamp, optional signing (a closed section "Signing (only for an updater that checks signatures)": a secret key file and CheckVersion; a patch is built unsigned unless you use it, and the result line says "unsigned" or "signed"). Warnings match the CLI (the original updater only fetches `psNNNN.patch`; `remove.txt` needs the toolkit or a replacement updater). After a build: Show folder, Apply to a client..., Verify.

**Apply & history.** Add patches (drop or Add...; drag to reorder), see for each its structure check and signature state, set a public key and CheckVersion (stored per client; "Require a valid signature" turns on with a key), read the plan (added, replaced, removed, missing, loose files, bytes to append), and **Apply N patches...**. The lower half is the history of journals for this client with the rollback state and the reason when blocked; **Roll back...** undoes the newest possible one; **Prune...** keeps only the newest N.

**Sign & verify.** Optional: the original updater does not check signatures. Generate a key pair (secret first; existing files are refused, never overwritten), sign patches or an updater executable (`.sig` next to each file; an existing one asks "Replace signature"), and verify signatures against a public key, optionally against a client (is the patch applied?). Shaiya Toolkit never stores key contents; the secret key's *path* is remembered only if you tick "Remember where my secret key is".

**SData.** Drop SData files or folders, or add all SData of the open client. The table shows encrypted or plain, checksum state and plaintext size. Decrypt to folder..., Encrypt to folder... (with "Match the originals' headers", which is what makes an unchanged file come out byte-identical) and Check. Outputs always go to a folder you choose; in-place conversion is a CLI feature.

**Inspect.** The file inspector: open or drop files, see what each is (SAH index, SAF data, SData encrypted or plain, patch, signature, `game.exe` build) with Name, Kind, Size and Details, the same text `shaiya info` prints. SHA-256 per row or for the selection runs in the background with progress in Activity and can be cancelled. Copy as text / Copy as JSON put the selected rows on the clipboard, handy for bug reports (never attach game files to a report; hashes are enough).

## Confirmations and prompts

Buttons say what they do and the risky one is marked. Every confirmation that changes a client folder shows the same four rows under its text:

| Row | What it says |
|---|---|
| **Changes** | The counts, taken from the plan that is applied (added, replaced, removed, loose files; for Compact the size before and after; for Roll back what is restored). |
| **Writes** | Each file or folder that changes, with sizes: `data.sah`, `data.saf` (how much is appended), the journal folder, loose files. |
| **Undo** | How to get back: "Apply & history > Roll back", or "no undo" (Compact makes no journal; Prune deletes journals; a rollback has none of its own). |
| **Game** | "must be closed (checked)" on Windows; elsewhere "cannot be checked on this system". |

| When | Text and buttons |
|---|---|
| Apply pending changes | "Apply N changes to NAME (folder)?" and the four rows. **Apply N changes** / Cancel. |
| Apply patches | "Apply PATCHES to NAME? Signatures: ..." where the signature state is "not used (unsigned patches)" when the client has no public key, "valid" when every patch verifies, "NOT CHECKED" when a key is set but the patches are not verified; then the four rows (one journal per patch). **Apply N patches** / Cancel. An invalid signature with "Require" on disables the button and says why. |
| Compact | "Compact data.saf of NAME?" and the four rows; Undo says "no undo: compacting makes no journal" and how many earlier changes can then no longer be rolled back. **Compact** / Cancel. |
| Roll back | "Roll back LABEL (applied TIME)?" and the four rows (index restored, `data.saf` cut back, loose files restored and deleted, the journal id). **Roll back** / Cancel. |
| Prune | Which journals are deleted and that those changes can then no longer be rolled back. **Prune** / Cancel. |
| Swap archives | "Put PAIR in place of NAME's data.sah/data.saf?" and the four rows (the current pair is renamed to `data.<UTC stamp>.sah/.saf`). **Swap archives** / Cancel. |
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
| Ctrl+Shift+P | Command palette: every action with its shortcut (Esc or a click outside closes it) |
| Ctrl+O, Ctrl+Shift+O | Open files, open by path |
| Ctrl+J | Activity panel (progress, cancel) |
| Ctrl+, | Preferences |

(On macOS read Ctrl as Cmd.)

## The shell

Menus: File, Edit, View, Tools, Help. **Tools** lists the command palette and the other toolkit programs that ship (Shaiya SVMap Editor); a program that is not in the folder next to the running one is shown as not installed. **Edit > Preferences** (Ctrl+,): theme (dark, light, system), UI scale, autosave and backup counts, the font used for Korean and Chinese names, whether extracted files keep the archive's pack time, and the Inspect column widths. **Help > About** shows the version, the git revision and the third-party notices; **Help > Open log file** and **Open data folder** open the toolkit's own files with your system's file opener.

**Help > Support the project** opens the author's donation page in your default browser. It happens only when you click it; Shaiya Toolkit makes no network connection of its own, and nothing is sent.

Korean and Chinese names need a font: Shaiya Toolkit looks for a system font at start (Malgun Gothic or Microsoft YaHei on Windows, Apple SD Gothic Neo or PingFang on macOS, Noto Sans CJK or a similar font on Linux). Name a font file in Preferences, or set `SHAIYA_TOOLKIT_FONT`, to use another one.

Settings and logs: see [install.md](../install.md#portable-mode-and-where-settings-live). Journals and backups are kept in each client folder, see [undo and recovery](../guides/undo-and-recovery.md).

## Left out on purpose

Dragging files out to the desktop, 3D model and animation previews, audio playback, text editing, SData record views (that is the planned SData Editor), several archives open at once (use Compare), in-place SData conversion, autosave of the pending queue (nothing is written before Apply, so a crash loses only the queue), and anything that uses the network.
