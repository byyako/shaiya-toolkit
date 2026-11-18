# Getting started in five minutes

You will look inside a client's archive, extract a file, build a small patch and apply it to **a copy** of your client, then undo it. Everything below is typed literally; paths are examples, so use your own folders. Nothing here touches your real client.

## Before you start

- A Shaiya client folder of your own. You need a **copy** to practise on: copy the whole folder in your file manager (for example `C:\Games\Shaiya` to `C:\Games\ShaiyaPractice`). The toolkit contains no game files and downloads none.
- The `shaiya` program from the [releases page](https://github.com/byyako/shaiya-toolkit/releases) (see [install.md](install.md)). Unzip it anywhere, open a terminal in that folder (on Windows: Windows Terminal, PowerShell or `cmd`), and check it runs:

```
shaiya --version
```

On Windows the first run may show SmartScreen ("Windows protected your PC"): choose More info, then Run anyway. If `shaiya` is not found, run it as `.\shaiya` from the folder you unzipped it to.

- Close the game, and anything else using the practice folder.

## 1. Save the practice client under a name

```
shaiya clients add practice C:\Games\ShaiyaPractice --note "practice copy"
```

From now on `"@practice"` means that folder in every command. `shaiya clients list` shows what is saved. (Shaiya Toolkit uses the same list.)

**Keep the double quotes around `"@practice"`.** In PowerShell a bare `@practice` is not text (the `@` makes PowerShell try to splat a variable), so the command fails with a confusing "required arguments were not provided" error. Double quotes work the same way in PowerShell and in `cmd.exe`, so every command on this page can be pasted into either. (Single quotes, `'@practice'`, work in PowerShell only; `cmd.exe` would pass them on literally.) The same goes for patterns such as `"*.SData"`.

## 2. Look inside

```
shaiya info C:\Games\ShaiyaPractice\data.sah
shaiya archive stats "@practice"
shaiya archive list "@practice" "*.SData" --long
shaiya archive check "@practice"
```

`info` says what a file is. `stats` counts files and unused space. `list` shows files (here every SData file; patterns are case-insensitive and `*.SData` matches in any folder). `check` looks for problems and ends with a line like `OK: 0 problems, 0 warnings`. None of these change anything.

## 3. Extract a file

```
shaiya archive extract "@practice" "*.SData" -o C:\work\extracted
```

The files appear under `C:\work\extracted` in their archive folders. If any of them already exist the command stops and tells you; add `--force` to overwrite or `--skip-existing` to leave them.

## 4. Build a patch

A patch is a ZIP with the files you want to add or replace. Make a folder whose layout mirrors the archive. Here a new text file in a new archive folder, so nothing existing is replaced:

```
mkdir C:\work\mod\shaiya-test
echo hello > C:\work\mod\shaiya-test\hello.txt
shaiya patch build -o C:\work\ps0001.patch --files C:\work\mod --dry-run
shaiya patch build -o C:\work\ps0001.patch --files C:\work\mod
shaiya patch list C:\work\ps0001.patch
```

The first `build` is a dry run: it prints what would go in and writes nothing. The second writes `ps0001.patch`. The updater only downloads files named `psNNNN.patch`, so patches you publish use that name. (To replace an existing file instead, put your edited copy at the same path, for example `C:\work\mod\Item\Item.SData`.)

## 5. Apply it to the practice copy

```
shaiya patch apply "@practice" C:\work\ps0001.patch --dry-run
shaiya patch apply "@practice" C:\work\ps0001.patch
shaiya archive list "@practice" "shaiya-test/*"
shaiya patch verify C:\work\ps0001.patch --client "@practice"
```

The dry run shows what would happen, including the free disk space needed. The real run prints `applied ps0001.patch: ...` and a `backup:` line naming the journal folder. `list` now shows `shaiya-test/hello.txt`, and `verify --client` says `applied to the client: yes`.

If it stops with `data.saf is open in another program`, close the game and anything else that has the folder open, then run it again; nothing was changed.

You will see `signature not checked`: patches can be signed so that only your own are accepted; see [make a patch](guides/make-a-patch.md) for that.

## 6. Undo it

```
shaiya patch history "@practice"
shaiya patch rollback "@practice"
shaiya archive list "@practice" "shaiya-test/*"
```

`history` lists the change and says `rollback possible`. `rollback` restores the old index, cuts `data.saf` back to its old length and removes anything the patch created. The last `list` finds nothing. Journals live in `C:\Games\ShaiyaPractice\shaiya-toolkit-backups`; [undo and recovery](guides/undo-and-recovery.md) explains what is kept and what can block a rollback.

## The same in Shaiya Toolkit

Run `shaiya-toolkit`. Pick the client in the toolbar (the list from step 1 is already there). The **Archive** tab lists and previews files; drop a file on it, check the target folder in the dialog that opens, press Queue changes, then **Apply to client...** (nothing is written before that). Open **Apply & history** to see the change and **Roll back...**. See [the Shaiya Toolkit page](tools/toolkit.md).

## Where next

- Changing game files for a mod: [guides/mod-files.md](guides/mod-files.md)
- Publishing patches for players: [guides/make-a-patch.md](guides/make-a-patch.md)
- SData tables: [guides/sdata-files.md](guides/sdata-files.md)
- Something went wrong: [guides/undo-and-recovery.md](guides/undo-and-recovery.md) and the [FAQ](faq.md)
- Every command: [tools/shaiya.md](tools/shaiya.md)
