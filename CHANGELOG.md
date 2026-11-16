# Changelog

All notable changes are listed here, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). One version number covers every program and library of the workspace.

## [Unreleased]

First foundation of the toolkit (no release yet).

### shaiya (CLI)

- New `shaiya` program with the global flags `--json`, `-q/--quiet`, `-v/--verbose`, `--progress`, and the exit codes documented in `docs/tools/shaiya.md`.
- `shaiya info [--hash] <PATH>...`: identifies SAH indexes, SAF data, SEED-encrypted and plain SData, patches and their signature files, and Windows programs (machine, link time); checks the SEED checksum; optional SHA-256 with progress.
- `shaiya version` (text and `--json`).

### Workbench

- New desktop program on the shared GUI shell: drop or open files, see what they are, compute SHA-256 as cancellable background tasks, copy a row as text or JSON.
- Shell: menus, command palette (Ctrl+Shift+P), Activity panel (Ctrl+J) with progress and cancel, error banner with copyable details, dark/light/system theme, UI scale, recent files with pins, settings that keep unknown keys, CJK font fallback from the system.

### Libraries

- `shaiya-core`: one error type, jobs with progress and cancel, atomic writes and backups (`plan_output`), a bounds-checked reader, text encodings, hashing, UTC time.
- `shaiya-seed`: SEED decrypt **and encrypt**, CRC-32, and a wrapper that round-trips SData files byte for byte; known-answer tests from a Python reference.
- `shaiya-archive`: index parsing and editing, `Archive` read access, compaction as a cancellable job. Moved from the Shaiya client research code.
- `shaiya-patch`: building, listing and applying `psNNNN.patch` files, Ed25519 signatures (message format unchanged). `apply` now refuses unsafe loose file names before it changes anything.
- `shaiya-clientdata`: read-only readers for client data (archive or folder, `.wld`, `.dg`, `.smod`, SData Monster and NpcQuest, sysmsg, textures); public API unchanged.
- `shaiya-inspect`: identify and describe a file.
- `shaiya-ui`: the GUI shell with a headless test harness. `shaiya-testkit`, `shaiya-build`, `shaiya-matrix-tests` and `xtask` support the build.
