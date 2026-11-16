# Patches: psNNNN.patch

A patch is an ordinary **ZIP** (deflate). The `shaiya-patch` crate builds, lists and applies them.

## Contents

| Entry | Meaning |
|---|---|
| `update.sah` + `update.saf` | A small archive in exactly the `data.sah` / `data.saf` format ([archive.md](archive.md)); offsets are relative to `update.saf`, and there is no free list. |
| `remove.txt` (optional) | Archive paths to drop, one per line. Lines starting with `#` and a header line `path` are skipped; only the first tab-separated column of a line is used, so a TSV report can be used as it is. |
| any other file | A **loose file** copied into the client folder as it is (for example a new `game.exe`). |

`update.sah`, `update.saf` and `remove.txt` are reserved names; a loose file may not use them.

## Applying a patch

`shaiya_patch::apply(patch, client, protect)` merges a patch into `client/data.sah` and `client/data.saf`:

1. Every loose file name is checked first (`enclosed_name`: no `..`, no absolute paths); an unsafe name refuses the whole patch before anything changes.
2. `update.saf` is unpacked next to the archive, never more than the patch's own index refers to (a guard against decompression bombs).
3. The listed removals are applied first, so a patch that lists a path and also carries it ends with its new copy. A listed path the archive does not have is only reported.
4. The new files are **appended** to `data.saf` and synced; entries with an existing name are replaced in place.
5. The previous index is copied to `data.sah.bak`, and the new index is written atomically (temporary file, then rename).
6. Loose files are written atomically. A name listed in `protect` (for example a running updater) is written as `<name>.new` instead.

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
