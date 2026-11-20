# The toolkit's own files in a client folder

When Shaiya Toolkit or `shaiya` changes a client folder it can leave a few files of its own there. This page lists every one of them: what it is called, what it holds, which action writes it, and what removes it. The files are not the game's formats; nothing here is read by the game. The guide [Undo and recovery](../guides/undo-and-recovery.md) says what to do with them; this page says what they are.

Except the journal folder and the files written next to a loose target (`.shaiya-tmp`, `.new`, `.bak`), these are plain files next to `data.sah`. The journal and the pending marker are small text files of `key=value` lines; the other files are copies, renames or the archive pair itself.

## At a glance

| Name | Where | What it is | Written by | Removed by |
|---|---|---|---|---|
| `shaiya-toolkit-backups/<stamp>-<label>/` | the client folder (a folder you name with `--journal-dir` replaces it) | one **journal**: the backup of one change | every in-place add/replace/remove, patch apply and Shaiya Toolkit's Apply, unless `--no-journal` (compaction and swap make none) | `shaiya patch history --prune N` and the Apply tab's prune; recovery of an unfinished change |
| `data.toolkit-pending` | the client folder | the **pending marker** of a change or rollback in progress | the same changes, before the first append | the change itself when it ends; recovery |
| `data.sah.bak` | next to `data.sah` | a plain copy of the previous index | in-place archive changes that replace the index, and `archive compact --in-place` | nothing; the next such change overwrites it |
| `data.sah.compact`, `data.saf.compact`, `data.compact-commit` | the client folder | the new archive pair and the commit marker of a compaction | `archive compact --in-place`, Shaiya Toolkit's Compact | the compaction; recovery |
| `data.swap-journal` | the client folder | the four planned renames of an archive swap | `archive swap`, Shaiya Toolkit's "Use another archive pair" | the swap; recovery |
| `data.<stamp>.sah`, `data.<stamp>.saf` (with `-1`, `-2` ... after the stamp when that name is taken) | the client folder | the client's own pair, kept by a swap | the same swap | nothing |
| `.<name>.<pid>.shaiya-tmp` | next to the file being written | a temporary file for an atomic write | every atomic write of the toolkit | the write, on success or failure |
| `<name>.new` | next to the target | a loose file the patch was told not to overwrite | `shaiya patch apply --protect NAME` | a rollback of that patch |
| `<name>.bak` (`<name>.<stamp>.bak` when `<name>.bak` already exists, then with `-1`, `-2` ... after the stamp) | next to the file | the original of a file changed in place | `sdata ... --in-place`; a forced rollback, for each loose file you changed | nothing |

## Journals: `shaiya-toolkit-backups/`

Each change gets its own folder `<UTC stamp>-<label>/` under `shaiya-toolkit-backups/` in the client folder. The stamp is `yyyymmddThhmmssZ`. The label is the patch or change name made into a short slug (the last extension, up to six letters or digits, is dropped, ASCII letters are lower-cased, every other run of characters becomes one `-`, leading and trailing `-` are removed, at most 40 characters; `ps0109.patch` becomes `ps0109`, an empty one `change`). When the folder already exists a counter is added (`-2`, `-3`, ...).

| Entry | Content |
|---|---|
| `journal.txt` | the record of the change, below |
| `data.sah` | a copy of the index as it was before the change |
| `loose/<path>` | a copy of each loose file the change overwrote, under the path it had in the client folder |

`journal.txt` is text, one `key=value` per line:

| Key | Meaning |
|---|---|
| `format` | `1` |
| `id` | the folder name |
| `kind` | `patch` when the label ends in `.patch`, else `archive-edit` |
| `label`, `created_utc`, `tool` | what the change was called, when (ISO 8601 UTC), and `shaiya <version>` |
| `sah_name`, `saf_name` | the file names of the pair as found in the client folder |
| `saf_len_before`, `saf_len_after` | the length of `data.saf` before and after |
| `sah_sha256_before`, `sah_sha256_after` | SHA-256 of the index before and after |
| `state` | `pending` (written first), `applied` or `rolled-back` |
| `rolled_back_utc` | when it was rolled back, else empty |
| `loose` (repeated) | tab separated: `1` if the file existed before or `0`, the SHA-256 it has after the change or `-`, the backup path inside the journal or `-`, and the path in the client folder |

A journal is what a rollback reads: it restores the index copy, cuts `data.saf` back to `saf_len_before` and puts the backed-up loose files back. Whether a rollback is still possible depends on the newer journals, the hashes and `data.saf`'s length: if `data.sah` is no longer the file the change wrote (compaction, a swap, the game's updater), the rollback is blocked and `history` says why. A rolled-back journal stays and is marked `rolled-back`.

Patches and edits cannot target a path inside the backup folder: such a name is refused.

## The pending marker: `data.toolkit-pending`

The marker exists only while a change or a rollback is running, or was interrupted. It is written before the first byte is appended to `data.saf` and removed when the change ends. It holds:

| Key | Meaning |
|---|---|
| `format` | `1` |
| `op` | `apply` or `rollback` |
| `label` | what the change was called |
| `saf_len_before` | the length `data.saf` must be cut back to |
| `sah_sha256_before`, `sah_sha256_after` | SHA-256 of the index before and after the change |
| `journal` | the absolute path of the journal folder, or `-` with `--no-journal` |
| `staged` (repeated) | `staged file`, a tab, `target`: loose files written beside their targets and not yet renamed into place |

Recovery runs before every write to a client and when Shaiya Toolkit opens a client. It compares the SHA-256 of the current `data.sah` with the two in the marker; what it does depends on `op`.

For `op=apply`: if `data.sah` is the one before, the change did not reach the index: `data.saf` is cut back, the staged files and the journal folder of the unfinished change are deleted and the marker is removed. If it is the one after, the change is finished (staged files renamed, the journal marked `applied`, the marker removed). If it is neither, recovery stops with an error and leaves the marker; so does a marker that cannot be read.

A change that writes only loose files leaves the index as it was, so the two hashes in its marker are equal and say nothing about how far it got. Then the loose files decide. If any staged file is already gone (renamed over its target), or the journal says `applied`, a target may already be overwritten: recovery finishes the change (renames the staged files that remain, marks the journal `applied`, removes the marker) and keeps the journal, because its `loose/` copies are the only backups; `shaiya patch rollback` can still undo the change. Only when every staged file is still in place and the journal is not `applied` is the change treated as not started: the staged files and the journal folder are deleted and the marker is removed.

For `op=rollback` (the hashes are those of the rollback: before is the changed index, after is the restored one): if `data.sah` is still the changed index, the marker is simply removed, because the rollback had not started; if it is the restored index, the rollback is finished (`data.saf` cut back, loose files restored, the journal marked `rolled-back`, the marker removed). For a change that wrote only loose files the two hashes are equal and say nothing about how far the restore got, so the journal decides: while it is still `applied` the rollback is finished (every loose file restored from the journal's `loose/` copies, files the change created removed, the journal marked `rolled-back`, the marker removed last). A loose file whose bytes are neither the change's nor the backup's, because something else changed it after the crash, is first saved as `<name>.bak` and the note names it. If the journal is missing, recovery stops with an error and leaves the marker. An unknown `op` stops recovery with an error.

## `data.sah.bak`

A plain copy of the index from before the latest change. It is written when an in-place change replaces the index (the Archive tab's Apply, `archive add|replace|remove --in-place` and `patch apply`) and by `archive compact --in-place`. A change that only writes loose files leaves it alone. It is overwritten every time, and the toolkit never reads it; it is a manual fallback. A `--no-journal` change keeps no other record of the old index.

## Compaction and swap files

A compaction writes `data.sah.compact` and `data.saf.compact`, then creates `data.compact-commit`, then renames the two over the originals and removes the commit marker. Recovery finishes the renames when the commit marker exists and deletes the two `.compact` files when it does not, so the index and `data.saf` always belong together. Compaction cannot be rolled back and ends rollback for every journal made before it.

A swap writes `data.swap-journal` (one line per rename: the old path, a tab, the new path), renames the client's own pair to `data.<stamp>.sah` and `data.<stamp>.saf` and the other pair into place, and removes the journal. Recovery repeats every rename whose source still exists and whose target does not, then removes the journal, so a crash at any point rolls forward. The kept pair stays where it is; swap it back with the same command.

## Temporary files and `.new`

Atomic writes go to `.<name>.<pid>.shaiya-tmp` in the folder of the target, are flushed, and are renamed over it. A failed or cancelled write removes the temporary file. A process that is killed leaves it. Shaiya Toolkit sweeps `*.shaiya-tmp` files older than a day from its own data folders, not from client folders. A patch apply also writes `.update.saf.<pid>.shaiya-tmp`, the unpacked archive data of the patch, and removes it when it ends.

A patch's loose file that you protect (`patch apply --protect NAME`) is written as `NAME.new` and the original is left alone; Shaiya Toolkit's Apply does not protect any name.

## What is not written into a client folder

[Publish](version-ini.md) writes only into a separate server folder; a destination that is a client folder or inside one is refused. `shaiya patch build`, the Patch builder and the table views write only to paths you name.

## Verified on

The names, keys and sequences above are those of the toolkit's code at this version and are covered by its tests (journal and marker round trips, interrupted apply, rollback, compaction and swap recovery on synthetic clients). The matrix rows `archive_edit_rollback_stub` and `patch_apply_rollback_stub` run an in-place change with a journal and its rollback on a copy of each client's real index on all nine matrix clients ([version matrix](../version-matrix.md)). A journal was not tried against a game running from the same folder.
