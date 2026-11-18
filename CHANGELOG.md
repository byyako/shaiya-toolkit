# Changelog

All notable changes are listed here, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). One version number covers every program and library of the workspace.

## [Unreleased]

### shaiya (CLI)

- `shaiya svmap`: `check`, `dump`, `build`, `roundtrip`, `diff`, `wld-check`, `spawns`, `bulk` for server map files and a client's `.wld` files, with the toolkit's exit codes and JSON.

### Shaiya SVMap Editor

- New desktop program `shaiya-svmap-editor` (ships in the Shaiya Toolkit zip): edits server map files (`.svmap`) in 2D and 3D over the client's terrain, with every section of the file, checks against the client and server data, paired client `.wld` edits written to a staging folder, placements and terrain, a Map.ini editor, Add a map, unsigned patch export, and Install and Uninstall on a test client. Verified on US EP4.5; other episodes are detected and the checks that depend on EP4.5 data say so. Its settings, log, recovery files and backups live in the `shaiya-svmap-editor` folder of the toolkit data folder. It has no subcommands: the command-line tools are `shaiya svmap`.

### Libraries

- `shaiya-clientdata`: `wld_raw` (lossless `.wld` model and writer), `area_txt` (line-preserving `world/<id>.txt`), `sdata::Gate` and `Npc.gates` (gatekeeper destinations).
- New `shaiya-svmap`: the `.svmap` server map format (model, reader, writer, validation, bit grid, JSON).
- New `shaiya-svmap-tools`: the SVMap editor's engine without a GUI (documents with undo, client pairing, checks, spawn report, bulk planner, Map.ini, new map, install) and the `batch` entry points of `shaiya svmap`.

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

### Shaiya SVMap Editor

- New desktop program `shaiya-svmap-editor` (ships in the Shaiya Toolkit zip): edits server map files (`.svmap`) in 2D and 3D over the client's terrain, with every section of the file, checks against the client and server data, paired client `.wld` edits written to a staging folder, placements and terrain, a Map.ini editor, Add a map, unsigned patch export, and Install and Uninstall on a test client. Verified on US EP4.5; other episodes are detected and the checks that depend on EP4.5 data say so. Its settings, log, recovery files and backups live in the `shaiya-svmap-editor` folder of the toolkit data folder. It has no subcommands: the command-line tools are `shaiya svmap`.

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
