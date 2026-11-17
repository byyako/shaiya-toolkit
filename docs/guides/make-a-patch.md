# Make a patch

For people who run a server and publish updates to their players: turn a folder of changed files, or the difference between two archives, into a `psNNNN.patch`, sign it, test it on a copy, and publish it as the next number.

A patch can add and replace files in `data.sah`/`data.saf`, remove archive files, and drop loose files into the client folder (for example a new `game.exe`). The format is in [formats/patch.md](../formats/patch.md). Everything below uses the [`shaiya patch`](../tools/shaiya.md#shaiya-patch) command; the Workbench's Patch builder tab does the same.

## 1. Get the inputs ready

Choose one way to describe the change.

**From a folder.** Make a folder that mirrors the archive: `changed/Item/Item.SData` becomes the archive file `Item/Item.SData`. Files that belong next to `game.exe` rather than in the archive go in a second folder (`loose/game.exe`). Archive files to delete go in a text file, one path per line:

```
shaiya patch build -o ps0110.patch --files changed --loose loose --remove remove.txt
```

All three inputs are optional, but at least one is needed.

**From the difference of two archives.** When you have the old and the new `data.sah`/`data.saf` (two client folders, or a stock pair and your edited one):

```
shaiya patch build -o ps0110.patch --diff C:\stock\data.sah C:\Games\Shaiya-new
```

builds the patch that turns the first into the second: files that are new or whose contents differ go in, files that disappeared become removals. Both sides must be full archives (with their `data.saf`), and the contents are compared, so this reads both archives completely. `--diff` may be combined with `--loose`. Look at the difference first with `shaiya archive diff OLD NEW --summary`.

**From the Workbench's pending changes.** Queue your edits in the Archive tab and press "Make patch...".

Add `--dry-run` to any `build` to see the contents table and sizes without writing a file.

## 2. Name it as the next number

Name the file `psNNNN.patch` with the next number after the one your clients are at (`CurrentVersion` in their `Version.ini`; `shaiya clients show NAME` prints it). The original updater only fetches that exact name, and `build` warns when it is something else. Patches must be applied in order, and `apply` warns when numbers are not ascending.

Two warnings you may see:

- *remove.txt is honoured by the toolkit and the replacement updater, not by the original Updater.exe.* If your players use the original updater, removals do nothing for them; replace files instead of removing them.
- The patch is empty.

The build is **deterministic**: the same inputs always produce the same bytes (fixed entry order and compression; entry times are the files' modification times, or one fixed time with `--timestamp 2026-11-15`). Rebuilding to check a patch gives an identical file, and a signature stays valid for it.

## 3. Check names

Build stops, listing every offender at once (exit 4), when a name cannot work:

- an archive path that cannot be written in code page 949 (for example an emoji), or one that is listed twice (two names differing only in case count as the same);
- a loose file called `data.sah`, `data.saf` or another file the toolkit manages, one inside `shaiya-toolkit-backups`, or one with a `..` or absolute path;
- a loose file whose name ends in `.shaiya-tmp`.

## 4. Sign it (recommended)

Signing lets a client or updater accept only patches that you made.

```
shaiya patch keygen --secret secret.key --public public.key
shaiya patch build -o ps0110.patch --files changed --sign secret.key --check-version 3
```

or sign an existing file: `shaiya patch sign ps0110.patch --key secret.key --check-version 3` (writes `ps0110.patch.sig`).

- `--check-version` is the client generation: the `CheckVersion` in the clients' `Version.ini`. The signature is bound to it, so patches made for an older generation cannot be replayed after you change it.
- The signature is also bound to the file name. A signed `ps0109.patch` is not valid as `ps0110.patch`.
- `keygen` never overwrites an existing key. **Keep `secret.key` off the patch server, backed up, in a folder only you can open.** Share only `public.key` with whoever checks patches. The toolkit prints the public key in hex too.
- **Never sign a different file under a number you have already published.** Clients that downloaded the first one would reject or mis-handle the second. Ship a fix as the next number.

## 5. Test on a copy

Never test on a client you care about. Copy a client folder, save it, and run the real thing:

```
shaiya clients add testcopy C:\Games\ShaiyaTest
shaiya patch verify ps0110.patch --key public.key --check-version 3
shaiya patch apply @testcopy ps0110.patch --key public.key --check-version 3 --dry-run
shaiya patch apply @testcopy ps0110.patch --key public.key --check-version 3
shaiya patch verify ps0110.patch --client @testcopy
shaiya archive check @testcopy --deep
```

`verify` checks the ZIP's structure and, with a key, the signature (`signature valid`; `MISSING` and `invalid` are failures with `--require-signature`, and `invalid` always is). `apply` with a key refuses any patch whose signature does not verify, before writing anything. `verify --client` confirms each file is present and identical, each removal is gone and each loose file matches. `check --deep` reads every file once. Start the game from the copy. If something is wrong, `shaiya patch rollback @testcopy` puts the copy back.

To test several patches in order, give them all to one command: `shaiya patch apply @testcopy ps0110.patch ps0111.patch ...`. Each is applied and journaled separately.

## 6. Publish

Upload `psNNNN.patch` (and `psNNNN.patch.sig` if you use signatures) to your patch server the way your updater expects, and raise the server's version number so it hands out the new patch. This toolkit does not upload anything and has no network code. Keep the exact files you published; if you need to prove which bytes went out, `shaiya info --hash ps0110.patch` prints the SHA-256.

## Common problems

| Message | What to do |
|---|---|
| `refused: ... problems:` followed by names | Fix every name listed and build again. |
| `output exists` | The patch file or its `.sig` already exists; choose another name or add `--force` (only for a number you have not published). |
| `signature check failed` / `signature invalid` | Wrong public key, wrong `--check-version`, a renamed file, or a changed file. |
| `data.saf is open in another program` on apply | Close the game and anything else using the folder. |
| `would overwrite the archive or a toolkit file` | A loose file has a reserved name. |

More: [undo and recovery](undo-and-recovery.md), [FAQ](../faq.md).
