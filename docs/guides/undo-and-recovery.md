# Undo and recovery

What the toolkit keeps when it changes a client folder, how to undo a change, what stops an undo, and what to do when a change was interrupted. This page is linked from the toolkit's own error messages (`data.toolkit-pending is damaged`, `data.swap-journal is damaged`).

## What a change does

Archive edits (`archive add|replace|remove --in-place`, Shaiya Toolkit "Apply to client") and `patch apply` all work the same way:

1. Finish or discard anything an earlier run left behind (see "Interrupted changes" below).
2. Check that the game or another program is not using the files (Windows).
3. Check everything it is about to do. Any problem stops it before a byte is written.
4. Write a **journal** and a **pending marker**.
5. **Append** the new file contents to the end of `data.saf` and flush them to disk. The old contents are never overwritten, so the old index still points at valid data.
6. Replace `data.sah` (the index) atomically.
7. Write loose files (files that go into the client folder itself), each atomically, after saving the originals into the journal.
8. Mark the journal applied and remove the pending marker.

If anything fails or you cancel before step 6, `data.saf` is cut back to its old length and nothing else has changed.

Shaiya Toolkit shows what a change will do before it does it: every confirmation that changes a client folder lists **Changes**, **Writes**, **Undo** and **Game** (see [the toolkit page](../tools/toolkit.md#confirmations-and-prompts)). The **Undo** row says whether the change can be rolled back and how, or "no undo".

## What is kept, and where

| Item | Where | Made by | Notes |
|---|---|---|---|
| Journal | `<client>/shaiya-toolkit-backups/<UTC stamp>-<label>/` (`--journal-dir` changes the folder) | every in-place change, unless `--no-journal` | `journal.txt`, a copy of the old `data.sah`, and a copy of each loose file the change overwrote. This is what rollback uses. |
| `data.sah.bak` | next to `data.sah` | in-place changes (including `--no-journal`), `compact --in-place` | A plain copy of the previous index. **Each change overwrites it**, so it holds only the index from before the *latest* change. It is a manual fallback; no command reads it. [Client health](client-health.md) lists it and can move it into a cleanup folder (never deleting it); it is not ticked at first. |
| Loose file backups | in the journal | the change that overwrote the file | **With `--no-journal` there is no backup of an overwritten loose file.** |
| `NAME.bak` | next to the file | `sdata ... --in-place` | The original; never overwritten (a time-stamped name is used when `NAME.bak` exists). |

`archive compact --in-place` is not journaled. It writes a new, smaller `data.saf` and a new `data.sah`, keeps the old index as `data.sah.bak` and **cannot be undone**: the old `data.saf` is gone. Compaction also ends rollback for every journal made before it (see below). Compact only when you are happy with the client as it is, or use `compact -o` to write a compacted copy instead.

Because step 5 only appends, `data.saf` grows with every change, and replaced or removed files leave unused bytes. `shaiya archive stats` shows how many; `compact` reclaims them.

## Undo the newest change

```
shaiya patch history "@us"
shaiya patch rollback "@us"
```

`history` lists every journal with its state (`applied`, `rolled back`, `pending`) and whether it can be rolled back right now. `rollback` undoes the newest one (`--id ID` names another, but only the newest applied journal can be undone). It:

- restores the old `data.sah` from the journal,
- cuts `data.saf` back to its length before the change,
- puts back the loose files the change overwrote and deletes the ones it created.

`--dry-run` says what it would do. In Shaiya Toolkit use the toolbar button **Undo last change**, Edit > **Undo last applied change (roll back)...** or **Apply & history > Roll back...**; all three open the same confirmation for the newest change, and when it cannot be rolled back they are greyed out with the reason as hover text. Rolling back several changes means rolling back the newest first, then the next.

The journal stays and is marked `rolled back`. `shaiya patch history --prune N` deletes all but the newest N journals; a pruned change can no longer be rolled back.

## What blocks a rollback

`history` prints the reason; `rollback` stops with exit 4 and the same text.

| Reason shown | Meaning and what to do |
|---|---|
| `roll back <id> first` | A newer change exists. Roll that one back first. |
| `data.sah changed since: compacted, swapped, patched by the updater or another tool` | The index is not the one this change wrote. Compaction, an archive swap, the game's own updater or another program changed it. This journal can no longer be undone. If you want the earlier state, restore a file backup or swap back (below). |
| `data.saf changed length since this change` | Something else wrote to `data.saf`. Same advice. |
| `the journal's copy of data.sah is missing or damaged` | The journal folder was edited or deleted. |
| `<file> was changed or removed since (use --force ...)` | A loose file the patch wrote has been modified. `--force` restores the original anyway and first saves your changed version under a `.bak` name. |
| `this change did not finish; open the client again ...` | State `pending`: see "Interrupted changes". |
| `already rolled back` | Nothing to do. |
| `data.saf is open in another program ...` | The game (or another program) has the files open. Close it and run the command again. |

## Interrupted changes

If the power fails or the process is killed during a change, a file named **`data.toolkit-pending`** stays in the client folder. The next toolkit command that writes to this client (and Shaiya Toolkit when it opens the client) reads it, works out how far the change got, and either undoes the unfinished change or finishes it, then says so on a `note:` line:

- `removed the unfinished change ...`: the index had not been replaced yet; `data.saf` was cut back. Nothing changed.
- `finished the interrupted change ...; shaiya patch history CLIENT shows it and shaiya patch rollback CLIENT can undo it`: the index was already replaced; the loose files were written and the journal marked applied.
- `finished the interrupted rollback of ...` / `an interrupted rollback of ... had not started`. When the change wrote only loose files, the journal decides: while it still says applied the rollback is finished, and a file you edited in between is saved as `<name>.bak` (or `<name>.<UTC>.bak` when that name exists) before it is overwritten or removed, and the note names it. If the journal folder is missing, recovery stops with an error and keeps `data.toolkit-pending`. If the journal already says rolled-back, the marker is removed with the note `an interrupted rollback of ... had already finished; the marker was removed`. A journal still marked applied that names a backup path outside its own folder is refused with an error and nothing is changed.

Otherwise there is nothing for you to do. The same applies to compaction (`data.sah.compact`, `data.saf.compact`, `data.compact-commit`) and archive swaps (`data.swap-journal`): they are completed or discarded automatically before the next write.

### When the marker cannot be resolved

Two errors need a decision by hand:

- `data.sah changed after an interrupted toolkit change; see data.toolkit-pending`: the index in the folder matches neither the state before nor after the change.
- `data.toolkit-pending is damaged; see docs/guides/undo-and-recovery.md`, or `data.swap-journal is damaged; ...`.

The marker is a short text file. Open it in a text editor and read `label=` (the change), `saf_len_before=` (the length `data.saf` had before the change), `journal=` (the journal folder) and `staged=` lines. To get back to the state before the change:

1. Make a copy of the whole client folder first, or at least `data.sah`, `data.saf`, `data.toolkit-pending` and the journal folder.
2. Close the game.
3. Copy `data.sah` from the journal folder over the client's `data.sah`. (If `journal=-`, the change ran with `--no-journal`; use `data.sah.bak` instead, which holds the previous index.)
4. Cut `data.saf` back to `saf_len_before` bytes. The toolkit's `patch rollback` does this for ordinary journals; by hand, a tool that truncates files (for example PowerShell: `$f=[IO.File]::OpenWrite("data.saf"); $f.SetLength(1991954802); $f.Close()` with your number) is needed.
5. Delete leftover `.shaiya-tmp` files and the `staged=` files named in the marker (hidden temporary names such as `.notice.txt.1234.shaiya-tmp`).
6. Delete `data.toolkit-pending`.
7. Run `shaiya archive check "@name"`; it should end with `0 problems`.

If you are unsure, restore your own backup of the client instead: the toolkit's journals are a safety net, not a replacement for a copy of anything you cannot get again.

## Swap back

`shaiya archive swap CLIENT OTHER` (or Shaiya Toolkit "Use another archive pair...") puts another `data.sah`/`data.saf` in place and keeps your pair as `data.<UTC stamp>.sah` / `data.<UTC stamp>.saf` in the same folder. The command prints the command that undoes it:

```
swap back with: shaiya archive swap "@us" C:\Games\Shaiya\data.20261115T120000Z.sah
```

Swapping needs both pairs on the same drive (it only renames files) and is crash safe: the four renames are written to `data.swap-journal` first and finished automatically next time. Journals made before a swap cannot be rolled back afterwards (the index is not the one they wrote); swapping back makes them possible again only if `data.sah` is byte-for-byte what they expect.

If `data.swap-journal` is damaged, open it: each line is `FROM<TAB>TO`, a rename to perform in order. Perform each rename whose source file exists and whose target does not, then delete `data.swap-journal`.

## Rules of thumb

- Keep your own copy of anything you cannot download again; journals are on the same disk as the client.
- Close the game before any change; Windows refuses, Linux and macOS cannot tell.
- After a rollback or a compaction, run `shaiya archive check "@name"`.
