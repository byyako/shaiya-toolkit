# Changelog

All notable changes are listed here, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). One version number covers every program and library of the workspace.

## [Unreleased]

### Shaiya Toolkit

- The Archive tab plays `.ANI` animations: it shows the kind and rig ("Character animation, rig huwm (Human)"), the bone count, end key and seconds at 30 keys per second, the key counts, a first field other than 0 labelled "inferred: start key", the validation errors and a count of warnings, a picture of the animation posed on a mesh (plain grey with wire) with the bones drawn as lines, Play/Pause, a slider, Loop, Bones, a Mesh choice (Skeleton only, Default parts, or one mesh) and a Bones table. The mesh is found by name in the `3DC` folder next to the `ANI` folder; a mesh is offered only when every bone it uses exists in the animation. The frames (at most 241 per animation) are prepared on a worker thread before playing. An approximation of the game's playback, validated offline; every file was read on US EP4.5 only, the eight other matrix clients have their index only.
- Add and "Replace with file..." check an `.ani` against the animation already in the client at the same path: a different skeleton asks once (**Add anyway** or **Cancel**) before the file is queued; an unreadable file, validation errors or a different first field (inferred: the start key; not checked in the game) only warn. Validated offline.
- The Archive tab previews `.3DC` skinned meshes at rest, in plain grey with the wire, with "Skinned mesh: N vertices, N triangles, N bones" (or the cloak and second-mesh headers). A `.3DC` or `.ANI` that does not read to the last byte shows its error and the bytes as hex; a `.3DO` with text-like bytes now stays hex with "no preview yet (the format is documented)" instead of showing as text.

### shaiya (CLI)

- Checks before files go in: `archive add` and `archive replace` also look at `.ani` files: `anim-unreadable`, `anim-invalid` and `anim-first-field-differs` (a different first field, inferred to be the start key, not checked in the game) warn, and `anim-rig-differs` (a different skeleton from the copy already in the client) warns too and never changes an exit code. A patch build is not rig-checked (no client copy). Validated offline.
- `shaiya anim info` describes `.ANI` animations and `.3DC` meshes (kind and rig of an archive path, bones, end key and seconds, key counts, vertices, triangles, validation errors and warnings; a first field other than 0 is shown as "inferred: start key"); `shaiya anim validate` prints each finding and exits 1 when a file has an error; `shaiya anim compare CANDIDATE TARGET` runs the rig check (same bone count, parents and reference matrices, plus the same first field, which is inferred to be the start key and not checked in the game, and no validation error) and prints `passes` or `does not pass`. `info` and `validate` take files, folders (searched for `.ani` and `.3dc` in any letter case) or `--from ARCHIVE` with patterns; `compare` takes a candidate file and a target file or, with `--from`, a target path in an archive; each has `--json`. Validated offline; every file was read on US EP4.5, the eight other matrix clients have their index only.

### Libraries

- `shaiya-clientdata`: `anim::Ani` reads a `.ANI` skeletal animation to the last byte and writes it back byte for byte (floats and NaN bits are kept exactly); `anim::validate` lists errors and warnings per bone (per file for `end-before-last-key`), `anim::rig_check` and `anim::passes` compare a candidate with the file it would replace (bone count, parents, reference matrices as raw bits, and the first field as a separate difference), and `anim::rig_of` names the kind and rig of an archive path. On US EP4.5, all 3,509 `.ANI` files read and write back byte for byte; the eight other matrix clients are index only (files counted, none read).
- `shaiya-clientdata`: `mesh3dc::C3dc` reads a `.3DC` skinned mesh in its three layouts (skinned, skinned plus a second mesh, cloak without a bone block) to the last byte, refuses a file that two layouts read, and writes it back byte for byte (floats, the vertex `pad` field and the raw texture-name bytes are kept). On US EP4.5, all 2,431 `.3DC` files read and write back byte for byte; the eight other matrix clients are index only (files counted, none read).
- `shaiya-clientdata`: pose and skinning for the preview. `anim::locals` and `anim::pose` compute the bone matrices of a `.ANI` at a time (row-vector math, slerp between rotation keys, linear translations, end keys held), `render::skin` poses a `.3DC` mesh by the vertex weights (a vertex that names a bone the animation lacks keeps its rest position and is counted), `render::c3dc_picture` draws a mesh at rest and `render::anim_frames` pre-renders up to 241 frames with one fixed view, a cancel check per frame and the bone origins for an overlay. `render::render_picture_at` takes the picture scale; `render_picture` gives the same pixels as before. An approximation of the game's playback, validated offline.
- `shaiya-clientdata`: `vani::Vani` reads a `.VANI` vertex-animated model (items, steps, per-step vertices, the `frame_ms` field, whose meaning is inferred) to the last byte and writes it back byte for byte; a collision flag other than 0 or 1 is refused, and the collision branch is read from a synthetic file only (no stock file uses it). `mani::Mani` reads a `.MANI` (exactly 108 bytes, any other size is refused) as raw bytes with 20 typed fields whose meanings are all inferred and were not checked in the game. On US EP4.5, all 66 `.VANI` and all 23 `.MANI` files read and write back byte for byte, and every `.MANI` the nine matrix indexes list is 108 bytes; the eight other matrix clients are index only (files counted, none read). Neither the Archive tab nor `shaiya anim` shows them yet.
- Version matrix: new rows for `.VANI` and `.MANI` (index counts for the nine clients, reads and writes back on US EP4.5, every listed `.MANI` is 108 bytes); format pages `vani.md` and `mani.md`.
- `shaiya-clientdata`: `mesh3do::Mesh3do` reads a `.3DO` item mesh (vertices, faces, an inferred frame rate and optional vertex frames) to the last byte and writes it back byte for byte; `mesh3de::Mesh3de` reads a `.3DE` effect mesh in its two layouts (with a frame block, or ending after the faces), accepts a file only when exactly one layout reads it, refuses any other file with "does not follow the .3DE layout the toolkit reads", and writes it back byte for byte. Floats and texture-name bytes are kept exactly; the meanings of the frame fields are inferred and were not checked in the game. On US EP4.5, all 271 `.3DO` files (one has frames: 11 at 20 per second) read and write back byte for byte, and 467 of the 469 `.3DE` files do; the other 2 (`004.3de`, `Deathknight007.3DE`) are refused and named on the format page. The eight other matrix clients are index only (files counted, none read). Neither the Archive tab nor `shaiya anim` shows them yet.
- Version matrix: new rows for `.3DO` and `.3DE` (index counts for the nine clients, reads and writes back on US EP4.5); format pages `3do.md` and `3de.md`; the two extensions are no longer listed as not documented.
- `shaiya-testkit`: `anim` builds synthetic `.ANI` bytes and synthetic `.3DC` bytes (a skinned mesh, a cloak, a small arm mesh) without using the reader; `env_client_original` names the optional `SHAIYA_CLIENT_ORIGINAL` archive.
- `shaiya-testkit`: the demo client also holds a made-up three-bone arm animation (`Demo/ANI/demo_wave.ANI`) and its mesh (`Demo/3DC/demo_arm.3DC`), built from `anim`; the screenshot scene `anim-preview` shows the Archive tab's player on it, paused at key 30. The documentation pictures were retaken on the demo client (10 files).
- `cargo xtask guard` also refuses tracked files ending `.ani`, `.3dc`, `.3do`, `.3de`, `.vani`, `.mani` and `.smod`.

## [0.3.2] - 2026-11-20

0.3.1 was never tagged or published: its changes are listed under [0.3.1] below and ship with this release.

### Shaiya Toolkit

- The SData table view says where its layout comes from. A table read with a layout that no client's own loader was read for shows "FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly."; a table read with a loader-verified layout shows "Loader-verified on: <clients>" after the existing note. The Archive tab's View table is the same window.
- New FIT-ONLY table layouts read files that were shown as hex before: `Item.SData` of kr-ep8 and fawkes-v68 (`item-134`), `Monster.SData` of kr-ep8 and fawkes-v68 (`monster-36`) and `NpcQuest.SData` of kr-ep8 and fawkes-v68 (`npcquest-287`). Their columns are named by position.
- The Archive tab previews `world/<id>.wld` maps: a field shows a grey hillshade top view (north up) with "Map <id>, <n> x <n> corners, <size> units" and its height range; a dungeon map shows "Dungeon map: the floor is in <name>; no top view yet". A `.wld` that does not parse shows the existing error with hex, whatever its first bytes are.
- The Archive tab previews `.smod` models: the textured, lit picture of the SVMap Editor's Models window with "N modules, N triangles, N textures (N missing)", textures looked up in the same archive under `Entity/Texture`; a missing texture is named and drawn grey. `.3DC` and `.3DO` models stay hex and say "no preview (format not documented yet)".
- Client health: **File > Client health...**, the **Health...** button in Manage client folders and a row on the Start page open a window that shows, for a saved client, the index check, dead space in `data.saf` (with a **Compact...** link), the journals (with **Prune...**), an unfinished change, `Version.ini` `StartUpdate=UPDATE_START`, leftover files and earlier cleanup folders. **Clean up...** (after the usual Changes / Writes / Undo / Game confirmation) moves the ticked leftovers into `shaiya-toolkit-backups/cleanup-<UTC>/` with a `manifest.json`; none of the leftovers is deleted. Before anything is moved, an interrupted change is finished or discarded by the same recovery every Toolkit write runs. **Put back...** moves them back. The window opens for the open client even when it is not a saved one, and a clean-up or Put back always acts on the client it was confirmed for. A temporary file that a pending change names, and the map editor's backups while its install record exists, are never offered.
- Several clients at once. Each client keeps its own queue of pending changes (in memory only), so switching the active client no longer needs the queue to be applied or discarded first. The Archive tab shows the active client's queue, the client picker marks a client that has queued changes ("US test - 3 queued"), quitting asks once and lists every client with a queue, and a queue is only ever applied to the client it was built on. Removing or renaming a client that has queued changes asks first and drops them.
- Compare takes any two saved clients or archive files: **OLD (left)** and **NEW (right)** pickers (the left side starts as the open client, **Swap sides** exchanges them), and **Make patch from differences** builds the patch that turns OLD into NEW. A comparison with no differences says "Nothing differs" and the patch button stays disabled.
- Patch builder: when "Leave out files that are already the same" leaves out every file (for example when the selected client is the NEW side of the differences) it says "Nothing differs: all N files are already the same in NAME ..." as a warning and as the reason Build is disabled, instead of only showing an empty patch.
- **Apply & history > Apply to several test clients...**: pick patches and saved clients; each client gets its own confirmation (Changes, Writes, Undo, Game), then the patches are applied to it with recovery first, the game-running check and its own journals, one client after the other. The run stops at the first client that fails (or when you answer **Stop here**) and lists the clients that were changed (with their journals) and the ones that were not touched.
- `shaiya-toolkit DEMO_DIR --shot OUT.png [--size WxH] [--scene NAME]` opens the real window on a synthetic demo client (`cargo xtask demo-client DIR`), saves a PNG of the window and exits with 0, or with 1 and a reason on standard error (2 for a bad flag). It reads and writes no settings unless `--data-dir` is given and shows no real path, version or date. It exists for the pictures of the docs; a scene refuses a folder without the demo marker `shaiya-demo-client.txt`, because `apply-history` makes one real change in it.

### shaiya (CLI)

- `sdata table` prints the same sentence on stderr (the FIT-ONLY one, or "Loader-verified on: <clients>."; also with `--csv` and with `--quiet`), and its JSON gains `status` (`loader-verified` or `fit-only`), `loader_verified_on` and `fit_only_on` next to `chosen_by` (also in the `--csv --json` summary). The existing keys are unchanged.
- `clients health NAME|DIR` looks at a client folder and writes nothing: the index check, dead space in `data.saf`, journals, a pending change, `Version.ini` `StartUpdate=UPDATE_START` and the leftovers; exit 0 when nothing is found, 1 when something is, 3 when the index cannot be read; with `--json` the report.
- `clients clean NAME|DIR --move KIND... [--dry-run]` moves leftovers (`sah-bak`, `stray-temp`, `swap-pair`, `editor-backup`, `editor-first-copy`) into `shaiya-toolkit-backups/cleanup-<UTC>/`; none of them is deleted, and an interrupted change is finished or discarded first by the same recovery every Toolkit write runs; `clients clean --put-back DIR` moves them back. Exit 4 when the game or another program holds the files, a pending change is still recorded after recovery, a rename fails partway, a destination exists or lies below a link, or the manifest names an unsafe path; exit 3 on other file errors, a manifest that cannot be rewritten after a move included (the message names the cleanup folder to Put back).

### Libraries

- `shaiya-sdata`: every layout file has a required `evidence` object (`loader`, `fit_only`: client ids); `parse_layout` refuses a layout without it. `Layout` gains `evidence`, `status()`, `provenance()` and `provenance_json()`; new `Evidence`, `Status` and `FIT_ONLY_NOTE`. Three layouts are added (`item-134`, `monster-36`, `npcquest-287`); `skill-9` also reads kr-ep8's `NpcSkill` file.
- `shaiya-sdata` `Table::to_json` gains `status`, `loader_verified_on` and `fit_only_on`.
- `shaiya-clientdata`: new `render` module with `shade_rect` (moved from `shaiya-svmap-tools`, which re-exports it; the editor's pictures are unchanged) and `hillshade_grey`.
- `shaiya-clientdata` `render` also holds the model picture moved out of the SVMap Editor (`render_picture`, `PreviewFit`, `PreviewTri`, `ModelPicture`, `MipImage`, `mip_chain`, `mip_image_capped`) and the new `model_picture`, which builds the preview of a parsed `Smod`; `texture_candidates` lists the paths a texture name can have. The editor's Models-window pictures are unchanged.
- `shaiya-archive`: new `health` module (`report`, read only) and `cleanup` module (`clean`, `clean_by`, `plan`, `put_back`, `list_folders`, `Manifest`); `shaiya-archive` now depends on `serde_json` (for `manifest.json`).
- `shaiya-ui`: the shell flags gain `--shot`, `--size` and `--scene` (`ShellOptions::shot`, `ShotOptions`); new `ToolApp::shot_scene` and `ToolApp::shot_step` (default: no scenes); new `Shell::hide_paths`, `Shell::folder_text` and `Shell::shot_inputs`; new `png` module (`png_encode`, moved from the SVMap Editor, which calls it) and `shot` module (`Idle`, `folder_label`); `Harness::wait_scene` for the tests; `Tasks::spawned`. `shaiya-ui` now depends on `miniz_oxide`, which the editor used before.
- `shaiya-testkit`: new `demo_client(dir)` and the pieces it is made of (`demo` module). It is a dev-only crate and is not published.

### Documentation

- New guide [Client health and clean-up](docs/guides/client-health.md); [the toolkit's files](docs/formats/toolkit-files.md) describe cleanup folders and the keys of `manifest.json`; the Shaiya Toolkit and `shaiya` pages describe the Health window and `clients health` / `clients clean`. The Monster.SData page says the columns of `monster-36` are `id`, `s0` and `f<offset>`.
- The version matrix says `FIT-ONLY <n>` (not `OK`) for the table fits that were not read from a client's loader (Item, Skill and NpcQuest on kr-ep8, fawkes-v68 and cn-ep4, Monster on kr-ep8 and fawkes-v68), has new Monster and NpcQuest rows, and records that cn-ep4 Monster has no fit. The Item, Skill, Monster and NpcQuest format pages, the SData files guide and the Toolkit and `shaiya` tool pages describe FIT-ONLY layouts and how a layout becomes loader-verified. The `.wld` and `.smod` format pages describe the Archive tab's previews, with new version matrix rows for the map and model preview data; the testing page describes the opt-in client health test; the Undo and recovery page points to Client health for `data.sah.bak`.
- The Shaiya Toolkit page, Mod files, Make a patch and Undo and recovery describe the per-client queue, Compare with two pickers, the "Nothing differs" message and Apply to several test clients.
- The Shaiya Toolkit pages show pictures of the demo client (`docs/assets/screenshots/toolkit-*.png`: Start, Archive, map and model previews, Patch builder, Apply & history, SData table, Client health, Inspect); [Editing these docs](docs/dev/docs-site.md#screenshots) says how to retake them.

## [0.3.1] - 2026-11-20

### Shaiya Toolkit

- DXT2 and DXT4 textures now preview and pass the texture check: they are read as DXT3 and DXT5 blocks and the colour is un-premultiplied. Before, such a file (`Item/DDS/reflection.dds` of the US EP4.5 client) was refused as `texture-unreadable`.
- Apply & history lists a change made outside the Toolkit (for example by the command line) when you switch to the tab or the window gets focus back.
- The Patch builder no longer waits for the comparison with the client when "Leave out files that are already the same" is unticked, and it compares again when the client's `data.sah` changed outside the Toolkit.

### Libraries

- `shaiya-clientdata::texture::decode_dds` reads `DXT2` and `DXT4`; the returned `Image` is straight-alpha RGBA like for every other format.

### Fixed

- A rollback of a change that wrote only loose files, interrupted by a crash, is now finished the next time Shaiya Toolkit opens the client or a command writes to it (the files it had not yet restored are restored). Before, the pending marker was dropped with "had not started", some files stayed restored and a second rollback was refused unless `--force` was given. A file changed by hand after the crash is saved first as `<name>.bak`, or `<name>.<UTC>.bak` when that name exists. A journal that already says rolled-back only has its marker removed, and a journal still marked applied that names a backup path outside its folder is refused.

### Documentation

- The texture format page, the version matrix, the recovery pages (Undo and recovery, the Toolkit's files) and the Toolkit page (Apply & history) describe the changes above; Make a patch notes that the Toolkit's "Leave out files that are already the same" works for every source while `shaiya patch build --diff` refuses `--only-changed`; the testing page explains temp folders on exFAT.

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
