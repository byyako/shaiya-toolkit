# Client health and clean-up

What the toolkit can tell you about a client folder, which leftover files it finds, and how it moves them aside. It looks first and changes nothing until you confirm. It deletes none of the leftovers it lists: it moves them into a folder of its own, and **Put back** moves them back. Before a clean-up, an interrupted change is finished or discarded by the same recovery every Toolkit write runs (step 1 below), and that recovery may remove its own temporary files.

## Where to find it

- Shaiya Toolkit: **File > Client health...**, the **Health...** button of a client in **File > Manage client folders...**, the **Check a client folder's health** row of the Start page, or the command palette. The window has a client picker (the saved clients) and **Check again**.
- Command line: `shaiya clients health NAME|DIR` and `shaiya clients clean` ([the commands](../tools/shaiya.md#shaiya-clients)).

## What it reports

| Line | What it says |
|---|---|
| Index | SAH version, file count, and how many problems and warnings the index check finds (the same check as `shaiya archive check`, without `--deep`). A missing `data.saf` is counted as a finding. |
| `data.saf` | Size on disk, bytes the index still uses, and the difference (dead space). The **Compact...** link opens the Archive tab's Compact confirmation when that client is the one open in the toolbar; otherwise it only says "Open this client first". Compacting makes no journal. |
| Journals | How many [journals](undo-and-recovery.md) there are and what they take. The **Prune...** link opens the Apply & history confirmation when that client is the one open in the toolbar; otherwise it only says "Open this client first". |
| A change that did not finish | `data.toolkit-pending` exists. Opening the client in Shaiya Toolkit, or the next write, finishes or removes that change. |
| `Version.ini` `StartUpdate=UPDATE_START` | An update by the original updater did not finish. This is explained and never edited. |
| Leftovers | Files below that can be moved aside, each with what it is. |
| Cleanup folders | Earlier clean-ups, with **Put back...**. |

## The leftovers

| Kind (CLI name) | What it is | Ticked at first |
|---|---|---|
| `sah-bak` | `data.sah.bak`, the only earlier index of a change made without a journal (and of a compaction). | no |
| `stray-temp` | `.<name>.<pid>.shaiya-tmp`, a temporary file from a write that did not finish, in the client folder or in a folder below it. A file that the pending marker names is **never** offered. If the marker cannot be read, no temporary file is offered. | yes |
| `swap-pair` | `data.<UTC stamp>.sah` and `.saf`, the pair kept by "Use another archive pair"; moved together. You need them to swap back. | no |
| `editor-backup` | `data.sah.svmap-editor-<stamp>`, an index copy made by the map editor's Install. | yes |
| `editor-first-copy` | `data.sah.svmap-editor`, the index from before the editor's first Install. | no |

The editor's two kinds are **not offered while `svmap-editor-install.json` is in the client folder**, because the editor's Uninstall uses those copies; they are listed as "not offered" with the reason. Nothing that an undo, a swap back or an uninstall may need is ticked at first.

## Clean up

**Clean up...** shows the usual confirmation with the four rows (Changes, Writes, Undo, Game; [what they mean](../tools/toolkit.md#confirmations-and-prompts)). After you confirm, the toolkit:

1. finishes or discards an interrupted change (the same recovery as before every write) and stops if a pending change is still recorded afterwards;
2. checks that the game or another program is not holding `data.sah` or `data.saf` (Windows; elsewhere it cannot tell, and the confirmation and the command say so);
3. makes `shaiya-toolkit-backups/cleanup-<UTC stamp>/` and writes `manifest.json` there with every file marked `planned` ([the file](../formats/toolkit-files.md#cleanup-folders-shaiya-toolkit-backupscleanup-utc-stamp));
4. renames the files into that folder one by one, rewriting `manifest.json` after each.

A rename that fails stops the run and says how many files were moved. Nothing is copied and deleted instead, so a clean-up only ever renames files within the client folder. The journals are not touched: a cleanup folder is not a journal and does not count as one.

## Put back

**Put back...** (or `shaiya clients clean --put-back DIR`) moves every file the manifest lists back to the name it had. If any of those names is taken again, nothing is moved and the names that are taken are listed; a destination below a symbolic link or junction is refused too. After a clean-up that was interrupted, Put back moves back whatever was moved; a file that was never moved is reported as already in place. The cleanup folder and its `manifest.json` stay. The manifest's state becomes `put-back` only when every file was restored or found in place; a file found in neither place stays listed as still to put back.

Put back only renames the leftover files of a closed cleanup folder back to their names. It does not run recovery, does not look for a pending change and does not check whether the game or another program holds `data.sah` or `data.saf`; close the game and let a pending change finish first. If it stops partway, the window says how many files are back and how many remain, and running it again finishes the rest.

## On the command line

```
shaiya clients health us
shaiya clients clean us --move stray-temp --dry-run
shaiya clients clean us --move stray-temp,sah-bak
shaiya clients clean --put-back "D:\Games\Shaiya\shaiya-toolkit-backups\cleanup-20261120T101500Z"
```

`health` exits 0 when nothing is found, 1 when something is, and 3 when the index cannot be read. `clean` exits 4 when the game or another program holds the files, a pending change is still recorded after recovery, an editor kind is requested while `svmap-editor-install.json` exists, or a rename fails partway through the clean-up (the message says how many files were moved and names the cleanup folder to Put back); with `--put-back` it exits 4 when a destination exists or lies below a link, or the manifest names an unsafe path (nothing is moved). It exits 3 on other file errors, among them a manifest that cannot be rewritten after files were moved (the message names the cleanup folder, so you can Put back). [All options and the JSON](../tools/shaiya.md#shaiya-clients).

## What it does not do

- It deletes none of the leftovers it lists, and does not edit `Version.ini` or touch `game.exe` or any DLL. The only removal is the recovery that runs first, which finishes or discards an interrupted change (it can remove that change's temporary files, such as `data.*.compact`, or cut `data.saf` back to its earlier length), as it does before every Toolkit write.
- Health never writes; it opens files for reading only.
- It does not find leftovers outside the client folder, and it does not scan for clients.
