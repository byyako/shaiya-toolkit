# Changelog

All notable changes are listed here, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). One version number covers every program and library of the workspace.

## [Unreleased]

### Shaiya Toolkit

- DXT2 and DXT4 textures now preview and pass the texture check: they are read as DXT3 and DXT5 blocks and the colour is un-premultiplied. Before, such a file (`Item/DDS/reflection.dds` of the US EP4.5 client) was refused as `texture-unreadable`.
- Apply & history lists a change made outside the Toolkit (for example by the command line) when you switch to the tab or the window gets focus back.
- The Patch builder no longer waits for the comparison with the client when "Leave out files that are already the same" is unticked, and it compares again when the client's `data.sah` changed outside the Toolkit.

### Libraries

- `shaiya-clientdata::texture::decode_dds` reads `DXT2` and `DXT4`; the returned `Image` is straight-alpha RGBA like for every other format.

### Fixed

- A rollback of a change that wrote only loose files, interrupted by a crash, is now finished the next time the client is opened (the files it had not yet restored are restored). Before, the pending marker was dropped with "had not started", some files stayed restored and a second rollback was refused. A file changed by hand after the crash is saved as `<name>.bak` first.

## [0.3.0] - 2026-11-19

### Shaiya Toolkit

- Patches are unsigned by default in the wording: the Patch builder keeps signing in a closed section "Signing (only for an updater that checks signatures)", a build says "unsigned", and Sign & verify starts with "Optional. The original updater does not check signatures."
- Apply & history says "Signatures: not used (unsigned patches)" when the client has no public key; "NOT CHECKED" now means a key is set but the patches were not verified.

- With no client or archive open, the Archive tab shows a Start page: one line for each job (add a client folder, change files, make a patch, apply patches) with a button that opens the right place.
- Undo last change: a toolbar button and Edit > Undo last applied change (roll back)... open the Roll back confirmation of the newest change the toolkit made; when that change cannot be rolled back (after Compact, for example) they are greyed out and the hover text says why. Ctrl+Z still undoes only the pending queue.
- Every confirmation that changes a client folder (apply changes, apply patches, compact, roll back, prune, swap archives) shows four rows under its text: Changes, Writes, Undo and Game, computed from the plan that is applied.
- Checks before files go in: the Add to archive dialog, Replace with file... and the Patch builder check each SData, `.dds` and `.tga` file and each empty file and list the findings (`sdata-double-encrypted`, `sdata-crc-bad`, `sdata-state-differs`, `texture-unreadable`, `empty-file`). Findings only warn, except that a file encrypted twice or with a failing checksum asks once, "Add anyway" (the Patch builder: "Build anyway") or Cancel; a check that cannot finish asks the same way instead of letting the files through.

- Publish for the regular updater: a section of the Patch builder (and a Start page row, "Publish a patch") copies patches into a server folder and raises `PatchFileVersion` in `UpdateVersion.ini`. Check the plan first (new files, the ini change, refusals, the file checks), then Publish asks with the Changes, Writes, Undo and Game rows; it never writes into a client folder.
- Patch from a mod folder: with a client selected, the Patch builder's box "Leave out files that are already the same in NAME" (on by default) leaves out the archive files that are identical to the client's copy; the contents table marks them "same, left out".

- SData tables: in the SData tab, select a file and press **View table** (or double-click) to open it as a table: virtual scrolling, a filter box, the row count, **Copy rows**, **Export CSV...**, a choice of code page for the names and, for `NpcQuest.SData`, of its tables. A file no layout fits is shown as hex under "No table layout fits this file (N bytes); shown as hex". The Archive tab's SData preview has the same **View table** button, and the Start page has a row "Look at SData tables". Read only. The view says how its layout was chosen ("chosen because it reads the whole file. It was checked against the game's own loader only on the clients listed as verified in the version matrix."), because the viewer does not know which client a file came from.

### shaiya (CLI)

- `sdata table INPUT [--from ARCHIVE] [--layout auto|item|skill|monster|npcquest] [--section NAME] [--csv OUT [--force]] [--code-page NAME]` shows an SData file as a table and writes it as CSV, or as JSON with `--json`; exit 3 when the file is unreadable, or no layout (or more than one) reads it to the last byte, 4 for an existing `--csv` output. Without `--csv` or `--json` the text output carries the same "chosen because it reads the whole file" note as the toolkit view, and the JSON has `"chosen_by": "fit"`.
- `patch build --files DIR --only-changed CLIENT` leaves out the files that are already identical (same length and SHA-256) in the client's archive, lists them on stderr and in the JSON `unchanged` array, and only reads the client.
- `patch publish PATCH... --to DIR [--check-version N] [--updater-version N] [--new-check-version] [--layout shaiya|flat] [--dry-run]` copies patches to `DIR/Shaiya/patch/` and raises `PatchFileVersion` in `DIR/Shaiya/UpdateVersion.ini` (exit 4 for a name that is not `psNNNN.patch`, a gap, a different file under a published name, a changed CheckVersion without `--new-check-version`, or a client folder as destination).
- `patch build`, `archive add` and `archive replace` print the same findings as `warning:` lines and in a `warnings` array of the `--json` result; exit codes do not change.

### Libraries

- New crate `shaiya-sdata`: SData tables as rows and columns. Layouts are data (`layouts/*.json`, one per format variant: `item-81`, `item-132`, `skill-3`, `skill-9`, `skill-15`, `monster-31`, `npcquest-165`, `npcquest-87`) read by one engine; a layout applies only when it reads the whole file to the last byte with plausible counts, and `read` with `Hint::Auto` takes the only one that fits.
- `shaiya-patch` has a `publish` module (`plan_publish`, `publish`, `parse_update_ini`, `set_update_ini_keys`) and `read_files`; `shaiya-inspect::validate::check_patch` checks the files inside a finished patch. `shaiya_archive::client::VersionIni` gained `start_update` and is no longer `Copy`.
- `shaiya-patch` has `BuildInput::skip_same_as` and `unchanged_files` (the comparison behind "only the changed files").
- `shaiya-inspect` has a `validate` module (`check_file`, `check_source`, `check_input`) behind the checks; it depends on `shaiya-clientdata` for the texture reader.

### Fixed

- Recovery of an interrupted change that wrote only loose files no longer deletes its journal. Such a change leaves the index as it was, so recovery could not tell a change that had not started from one that had already overwritten some files, and it removed the journal that held the only backups; now it keeps the journal and finishes the change when any staged file is gone or the journal says applied, so it can still be rolled back (tests `loose_only_change_crashed_inside_the_commit_keeps_the_journal_and_rolls_back`, `loose_only_change_crashed_before_any_target_changed_is_cleaned_up` and `loose_only_change_crashed_after_the_commit_keeps_the_journal` in `crates/shaiya-archive/tests/edit.rs`).

### Breaking

- `shaiya_ui::shell::Confirm` gained the field `details` (labelled rows under the text) and now implements `Default`; a `Confirm` written out field by field needs `..Default::default()`.
- `shaiya_archive::client::VersionIni` gained the field `start_update` and is no longer `Copy` (it is still `Clone`).
- `shaiya-inspect` now depends on `shaiya-clientdata` (new module `validate`), so a program that uses `shaiya-inspect` links it too.
- `shaiya_patch::BuildInput` has the new method `skip_same_as`; no existing field or method changed.

### Documentation

- New formats pages [Item.SData](docs/formats/item-sdata.md) and [Skill.SData](docs/formats/skill-sdata.md); the SData files guide has a section "Look at a table"; the CLI and toolkit pages describe `sdata table` and View table ("SData record views" is gone from "Left out on purpose"; editing stays); the version matrix has three rows measured on every test client that has the data (ru-ep54 is index-only): `Item` fits, `Skill` fits, `auto` picks exactly one layout or none.
- New page [Formats: overview](docs/formats/README.md): every format page with what reads it, what writes it and where it was verified, and a second table "Known, not yet documented" with the extensions the nine matrix clients hold that no page covers, counted read-only from their indexes, each marked "planned for" 0.4.0 animations, 0.5.0 effects or "not planned". A test (`every_format_page_is_listed` in `xtask`) fails when a page in `docs/formats/` is not linked from it.
- New page [The toolkit's own files in a client folder](docs/formats/toolkit-files.md): journals (`shaiya-toolkit-backups/`), the pending marker `data.toolkit-pending`, `data.sah.bak`, the compaction and swap files, temporary files, `NAME.new` and `NAME.bak`, with their keys, who writes them and what removes them.
- New page "Version.ini and UpdateVersion.ini" (keys, folder layout, what the original updater does with them, what `patch publish` writes); the make-a-patch guide section 6 describes publishing; the CLI page and the toolkit page describe `patch publish` and the Publish section.
- The toolkit page describes the Start page and the four confirmation rows; the undo-and-recovery guide points to them.
- The make-a-patch guide section 4 is now "Optional: sign it" (the anchor changed to `#4-optional-sign-it`) and the patch format page says signatures are optional.
- The make-a-patch guide ("Only the changed files"), the CLI page and the toolkit page describe leaving out files that are already the same in the client.
- The SData wrapper page describes a file encrypted twice; the CLI page, the toolkit page and the mod-files, SData files and make-a-patch guides list the checks before files go in; the version matrix gains two assumption rows with the measured numbers; the textures page says DXT2 and DXT4 DDS files do not read.

## [0.2.1] - 2026-11-19

### Shaiya Toolkit

- The command palette closes when you click outside it, as well as on Esc.
- Help > Open log file, Help > Open data folder and Show folder after a build open the file or folder with the system's opener (Explorer, Finder, xdg-open) instead of the web browser.
- The Tools menu lists Shaiya SVMap Editor, which ships with the toolkit, instead of programs that do not exist.

### Shaiya SVMap Editor

- File > Export may write a file in a folder that holds the client folder (for example the Desktop); a file in or at the client folder stays refused, also through a junction or symbolic link.
- The 3D view's Frame map fits the whole map with its height at the current window shape, so tall small dungeons are no longer cropped. New View menu entries: Top-down 3D view, Level 3D view and 45 degree 3D view.

### Documentation

- The episode claims are exact: there is no detection of the client version; every file is checked by whether it reads (version matrix, FAQ and the SVMap Editor guide).
- The guides no longer link seven screenshots of the Shaiya Toolkit and one of the Placements tool that are not published; a test in `cargo xtask` keeps every picture in the README and `docs/` pointing at a file in the repository.

### shaiya (CLI)

- `shaiya svmap check --client` (and `spawns`, `bulk`, `wld-check`) refuse a folder that holds no client files without walking it to the end: the look stops after 20,000 entries or 4 folders deep and exits 3.

### Release files

- The README.txt of the command-line zip names only what applies to a command-line program: on Linux the `chmod +x` line, on macOS the `xattr -d com.apple.quarantine` line, with no desktop, driver or app wording. The zip with the desktop programs keeps its text.

## [0.2.0] - 2026-11-18

### shaiya (CLI)

- `shaiya svmap`: `check`, `dump`, `build`, `roundtrip`, `diff`, `wld-check`, `spawns`, `bulk` for server map files and a client's `.wld` files, with the toolkit's exit codes and JSON.
- `shaiya svmap diff` exits 3 when a path is missing, and an empty `--client` folder exits 3; the server-data checks print notes when the server data is absent.

### Shaiya SVMap Editor

- New desktop program `shaiya-svmap-editor` (ships in the Shaiya Toolkit zip): edits server map files (`.svmap`) in 2D and 3D over the client's terrain, with every section of the file, checks against the client and server data, paired client `.wld` edits written to a staging folder, placements and terrain, a Map.ini editor, Add a map, unsigned patch export, and Install and Uninstall on a test client. Exports refuse the client folder and program targets (`.exe`, `.dll`). Verified on US EP4.5; other episodes are detected and the checks that depend on EP4.5 data say so. Its settings, log, recovery files and backups live in the `shaiya-svmap-editor` folder of the toolkit data folder. It has no subcommands: the command-line tools are `shaiya svmap`.

### Libraries

- `shaiya-clientdata`: `wld_raw` (lossless `.wld` model and writer), `area_txt` (line-preserving `world/<id>.txt`), `sdata::Gate` and `Npc.gates` (gatekeeper destinations).
- `shaiya-patch`: `number_hint` (next patch number from Version.ini and existing patch files).
- New `shaiya-svmap`: the `.svmap` server map format (model, reader, writer, validation, bit grid, JSON).
- New `shaiya-svmap-tools`: the SVMap editor's engine without a GUI (documents with undo, client pairing, checks, spawn report, bulk planner, Map.ini, new map, install) and the `batch` entry points of `shaiya svmap`.

### Documentation

- The user guide of Shaiya SVMap Editor (`docs/tools/svmap-editor.md`, also its Help window): what it writes, verified episodes and what was checked in the game, install and data folder on all three platforms, free space notes for Install.
- Fourteen formats pages: `.svmap` (with its JSON form), `.wld`, world area names, `Map.ini` and `MapWar.ini`, `Obelisk.ini`, `NpcQuest.SData`, `Monster.SData`, `.dg`, `.smod`, textures, `sysmsg-uni.txt` and map names, `TacticsZone.zon`, `mobs.psv`, and the editor's own files. Each has Verified on and Other episodes sections.
- The version matrix has a section "SVMap Editor and `shaiya svmap`" with the assumptions per episode. README, install, FAQ, getting started and the developer pages describe the second desktop program.

### Breaking

- `shaiya_clientdata::sdata::Npc` has a new field `gates`; code building `Npc` with a struct literal must set it.

## [0.1.0] - 2026-11-17

First release: the foundation of the toolkit and the archive and patch tools.

### shaiya (CLI)

- New `shaiya` program with the global flags `--json`, `-q/--quiet`, `-v/--verbose`, `--progress`, `--data-dir DIR`, and the exit codes documented in `docs/tools/shaiya.md`.
- `shaiya info [--hash] <PATH>...`: identifies SAH indexes, SAF data, SEED-encrypted and plain SData, patches and their signature files, and Windows programs (machine, link time); checks the SEED checksum; optional SHA-256 with progress. `shaiya version`.
- `shaiya archive`: `list`, `stats`, `check` (`--deep`, `--strict`), `index`, `extract`, `cat`, `add`, `replace`, `remove`, `compact`, `diff`, `pack`, `swap`. Case-insensitive glob patterns, `@name` for saved client folders, shadowed entries hidden unless asked for, `-o` for a new archive pair or `--in-place` on a client with a journal, `--dry-run`, JSON for every command.
- `shaiya patch`: `build` (from folders, loose files, removals or the difference of two archives; deterministic output; optional signing), `list`, `verify` (structure, signature, applied to a client), `apply` (several patches in order, signature required with a key, `--protect`, `--dry-run`), `rollback`, `history` (`--prune`), `keygen`, `sign`, `sign-updater`.
- `shaiya sdata`: `info`, `check`, `decrypt`, `encrypt` over files, folders or a client's archive (`--from`); `--like` makes an unchanged file encrypt back byte-identical and keeps plain files plain.
- `shaiya clients`: `list`, `add`, `remove`, `show`: the saved client folders shared with Shaiya Toolkit.
- Every change to a client folder is journaled in `<client>/shaiya-toolkit-backups/` and can usually be undone with `shaiya patch rollback`; interrupted changes are finished or discarded automatically. Writing refuses while the game has the files open (Windows). `--no-journal` keeps only the previous index as `data.sah.bak`; `archive compact --in-place` is not journaled and ends rollback of earlier changes.

### Shaiya Toolkit

- New desktop app on the shared GUI shell (executable `shaiya-toolkit`) with six tabs: **Archive** (folder tree, search, previews for images, text, SData and hex, extract, queued changes applied with confirmation, check, compare, compact), **Patch builder**, **Apply & history** (plan, apply, roll back, prune), **Sign & verify**, **SData** (decrypt and encrypt into a folder) and **Inspect** (identify files, SHA-256 as cancellable background tasks, copy as text or JSON).
- Saved client folders shared with the command line, "Open other archive..." read-only, swap archive pair with a way back, confirmation and Retry prompts, keyboard shortcuts for every tab and action, command palette.
- Shell: menus, command palette (Ctrl+Shift+P), Activity panel (Ctrl+J) with progress and cancel, error banner with copyable details, dark/light/system theme, UI scale, recent files with pins, settings that keep unknown keys, CJK font fallback from the system.
- Help > Support the project opens the author's donation page in the default browser, only when clicked.

### Libraries

- `shaiya-core`: one error type, jobs with progress and cancel, atomic writes and backups (`plan_output`), a bounds-checked reader, text encodings, hashing, UTC time; glob patterns and filters, byte sources, the saved client folders and the data folder.
- `shaiya-seed`: SEED decrypt **and encrypt**, CRC-32, a wrapper that round-trips SData files byte for byte, and batch check, decrypt and encrypt; known-answer tests from an independent reference.
- `shaiya-archive`: index parsing and editing (shadowed folders and free lists handled, offsets past 4 GiB), `Archive` read access, layout analysis and `check`, extract, in-place edits with journals, rollback and crash recovery, new-pair writes, compaction as a cancellable job, diff, pack, swap.
- `shaiya-patch`: streaming deterministic builds, listing, verification, planning, journaled apply, and Ed25519 signatures (message format unchanged). `apply` refuses unsafe loose file names before it changes anything; the original `apply` call is kept.
- `shaiya-clientdata`: read-only readers for client data (archive or folder, `.wld`, `.dg`, `.smod`, SData Monster and NpcQuest, sysmsg, textures); public API unchanged.
- `shaiya-inspect`: identify and describe a file.
- `shaiya-ui`: the GUI shell with a headless test harness. `shaiya-testkit` (archive, ZIP, image and PE builders), `shaiya-build`, `shaiya-matrix-tests` (nine new capability rows for the archive and patch tools, see `docs/version-matrix.md`) and `xtask` support the build.

### Documentation

- Pages for the command line and Shaiya Toolkit, a getting-started page, guides for modding files, making patches, SData files and undo and recovery, an FAQ, and the archive and patch format pages extended.
- A documentation site (MkDocs Material, offline-safe: no external fonts, scripts or analytics) built from `docs/` and deployed to GitHub Pages.
