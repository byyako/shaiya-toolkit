# Patches: psNNNN.patch

A patch is an ordinary **ZIP** (deflate). The `shaiya-patch` crate builds, lists and applies them.

## Contents

| Entry | Meaning |
|---|---|
| `update.sah` + `update.saf` | A small archive in exactly the `data.sah` / `data.saf` format ([archive.md](archive.md)); offsets are relative to `update.saf`, and there is no free list. |
| `remove.txt` (optional) | Archive paths to drop, one per line. Lines starting with `#` and a header line `path` are skipped; only the first tab-separated column of a line is used, so a TSV report can be used as it is. |
| any other file | A **loose file** copied into the client folder as it is (for example a new `game.exe`). |

`update.sah`, `update.saf` and `remove.txt` are reserved names; a loose file may not use them.

## Loose file names

A loose file is written into the client folder, so its name is checked when a patch is **built** and again when it is **applied**. A name is refused when it:

- is absolute, contains `..` or `.` segments, a drive colon, or an empty segment;
- is `data.sah`, `data.saf` or another file the toolkit manages (`data.toolkit-pending`, `data.compact-commit`, `data.swap-journal`, `data.sah.compact`, `data.saf.compact`), in the client folder itself;
- is inside the toolkit's `shaiya-toolkit-backups` folder;
- has a segment ending in `.shaiya-tmp`;
- differs from another loose file's name only in letter case.

Building lists every offending name at once. A patch that contains such a name is refused as a whole before anything changes.

## Removals

`remove.txt` is understood by this toolkit (`shaiya patch apply`, the Workbench) and by replacement updaters that implement it. **The original `Updater.exe` ignores it**, so a patch that depends on removals does nothing for players of the original updater; `shaiya patch build` warns about that. Removing a path the archive does not have is reported, not an error.

## Deterministic build

The same inputs always produce the same patch bytes: entries are sorted by name, the compression level is fixed, and entry times come from the files' modification times (or from one `--timestamp` for every file). Nothing in the file records the machine, the user or the build time. This makes a patch reproducible and makes its signature stable.

## Applying a patch

`shaiya_patch::apply(patch, client, protect)` merges a patch into `client/data.sah` and `client/data.saf`:

1. Every loose file name is checked first (`enclosed_name`: no `..`, no absolute paths); an unsafe name refuses the whole patch before anything changes.
2. `update.saf` is unpacked next to the archive, never more than the patch's own index refers to (a guard against decompression bombs).
3. The listed removals are applied first, so a patch that lists a path and also carries it ends with its new copy. A listed path the archive does not have is only reported.
4. The new files are **appended** to `data.saf` and synced; entries with an existing name are replaced in place.
5. The previous index is copied to `data.sah.bak` (a plain copy that the next change overwrites), and the new index is written atomically (temporary file, then rename).
6. Loose files are written atomically. A name listed in `protect` (for example a running updater) is written as `<name>.new` instead.

`shaiya patch apply` and the Workbench do the same through the journaled in-place procedure ([archive.md](archive.md#changing-an-archive-in-place)): before the first byte is appended they record a **journal** (a copy of the old index, a list of loose files and copies of those they overwrite) so that `shaiya patch rollback` can undo the patch. Journals live in the client folder under `shaiya-toolkit-backups/`; they are local backups made by the toolkit and are **not part of the patch format**. The older `shaiya_patch::apply` call (kept for updaters) makes no journal. Loose files written without a journal (`--no-journal`) leave no copy of what they overwrote. See [undo and recovery](../guides/undo-and-recovery.md).

If the process stops early, the old index still describes valid data; only unreferenced bytes are left at the end of `data.saf`. Replaced and removed files leave dead space, which `shaiya_archive::compact` reclaims.

Paths are Unicode in lists and patches and code page 949 in the index.

## Signatures

A patch can be accompanied by `psNNNN.patch.sig`, an Ed25519 signature (lower-case hex) over this message:

```
"shaiya-patch-v2\0" + CheckVersion (decimal) + "\0" + lower-case file name + "\0" + SHA-256(file)
```

- Binding the **file name** means a signed `ps0109` cannot be served as `ps0110`.
- Binding **CheckVersion** means patches of an older client generation cannot be replayed after the generation changes.
- An updater's own new version is signed the same way under the name `shaiya-updater-<UpdaterVersion>.exe`, so a patch signature is never valid for an updater and a signed updater cannot be offered under a higher version number.
- Keys are 32 bytes, signatures 64 bytes, both stored as lower-case hex text.

The message format does not change without a new message version: deployed updaters verify it. Rule for publishers: never sign a different file under a version number that was already published; ship fixes as the next version.
