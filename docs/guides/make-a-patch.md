# Make a patch

For people who run a server and publish updates to their players: turn a folder of changed files, or the difference between two archives, into a `psNNNN.patch`, sign it, test it on a copy, and publish it as the next number.

A patch can add and replace files in `data.sah`/`data.saf`, remove archive files, and drop loose files into the client folder (for example a new `game.exe`). The format is in [formats/patch.md](../formats/patch.md). Everything below uses the [`shaiya patch`](../tools/shaiya.md#shaiya-patch) command; Shaiya Toolkit's Patch builder tab does the same.

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

**From Shaiya Toolkit's pending changes.** Queue your edits in the Archive tab and press "Make patch...".

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

## Checks before files go in

Before an edit or a build, the toolkit looks at each file and prints what it finds as a warning; nothing is blocked. Shaiya Toolkit lists the same findings in the Add dialog and the Patch builder (and asks once for a file that is SEED-encrypted twice or whose checksum fails: **Add anyway** in the Add dialog, **Build anyway** in the Patch builder, **Publish anyway** in Publish). The full table is in [Checks before files go in](../tools/shaiya.md#checks-before-files-go-in).

- `sdata-double-encrypted`: the SData file was encrypted twice (its decrypted body is SEED-encrypted again). Decrypting it once with `shaiya sdata decrypt` gives the singly encrypted file.
- `sdata-crc-bad`: the SEED checksum fails or the wrapper is damaged.
- `sdata-state-differs`: you are putting a plain SData where the client's copy is encrypted, or the reverse. Some clients store theirs plain, so this is advice only; `sdata encrypt --like "@name"` takes the form from the client (see [SData files](sdata-files.md)).
- `texture-unreadable`: a `.dds` or `.tga` the toolkit's reader cannot open. The game may still read it; open it in an image editor to be sure.
- `empty-file`: the file has no bytes.

## 4. Optional: sign it

Optional. The original updater does not check signatures, so a patch works without one. Sign only if you run an updater that requires signed patches: signing lets it accept only patches that you made.

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
shaiya patch apply "@testcopy" ps0110.patch --key public.key --check-version 3 --dry-run
shaiya patch apply "@testcopy" ps0110.patch --key public.key --check-version 3
shaiya patch verify ps0110.patch --client "@testcopy"
shaiya archive check "@testcopy" --deep
```

`verify` checks the ZIP's structure and, with a key, the signature (`signature valid`; `MISSING` and `invalid` are failures with `--require-signature`, and `invalid` always is). `apply` with a key refuses any patch whose signature does not verify, before writing anything. `verify --client` confirms each file is present and identical, each removal is gone and each loose file matches. `check --deep` reads every file once. Start the game from the copy. If something is wrong, `shaiya patch rollback "@testcopy"` puts the copy back.

To test several patches in order, give them all to one command: `shaiya patch apply "@testcopy" ps0110.patch ps0111.patch ...`. Each is applied and journaled separately.

## 6. Publish

For the original updater a patch is published by two things on the web server: the file `Shaiya/patch/psNNNN.patch` and a higher `PatchFileVersion` in `Shaiya/UpdateVersion.ini` ([the files](../formats/version-ini.md)). `shaiya patch publish` prepares both in a folder, which you then upload the way you always do. The toolkit has no network code and uploads nothing.

```
shaiya patch publish ps0110.patch --to server-files --check-version 3 --dry-run
shaiya patch publish ps0110.patch --to server-files --check-version 3
```

- The first line shows the plan and writes nothing. The second copies `ps0110.patch` to `server-files/Shaiya/patch/`, raises `PatchFileVersion` in `server-files/Shaiya/UpdateVersion.ini` (a new file gets `CheckVersion`, `UpdaterVersion` 1 and the number) and reads every written file back. `--layout flat` puts the ini and the patches in the folder itself; `--updater-version N` sets `UpdaterVersion`.
- With an existing `UpdateVersion.ini` in the folder, `--check-version` can be left out: the file's value is kept. A different value is refused unless you add `--new-check-version`, because every player's updater then tells them to reinstall the game.
- Give several patches in one go (`ps0110.patch ps0111.patch`); they are published in number order. A number that leaves a gap after `PatchFileVersion` is refused, and so is a different file under a name that is already in the folder: a published patch is never replaced, ship the fix as the next number. Publishing the same file again changes nothing.
- A destination that is a client folder (a saved client, or a folder with `data.sah` and `data.saf` in or above it) is refused: this writes into a server folder only.
- Shaiya Toolkit does the same in the Patch builder, section **Publish for the regular updater** (also the Start page's **Publish a patch**): add the patches (or **Use the patch just built**), choose the server folder, **Check the plan**, then **Publish**. The confirmation shows the Changes, Writes, Undo and Game rows; there is no undo, and the Undo row names the ini values you would set back by hand.

Keep the exact files you published; if you need to prove which bytes went out, `shaiya info --hash ps0110.patch` prints the SHA-256. Players on the original updater ignore `remove.txt`, so a patch that only removes files does nothing for them (`publish` warns).

## Common problems

| Message | What to do |
|---|---|
| `refused: ... problems:` followed by names | Fix every name listed and build again. |
| `output exists` | The patch file or its `.sig` already exists; choose another name or add `--force` (only for a number you have not published). |
| `signature check failed` / `signature invalid` | Wrong public key, wrong `--check-version`, a renamed file, or a changed file. |
| `data.saf is open in another program` on apply | Close the game and anything else using the folder. |
| `would overwrite the archive or a toolkit file` | A loose file has a reserved name. |

More: [undo and recovery](undo-and-recovery.md), [FAQ](../faq.md).
