# Architecture

Every tool is a thin command-line or GUI layer over one shared, tested Rust core.

## Crates

```
crates/
  shaiya-core/        errors, jobs and progress, safe writes, binary reader, text encodings, hashing, UTC time
  shaiya-seed/        SEED cipher and the SData wrapper
  shaiya-archive/     data.sah / data.saf: index, reading, add/replace/remove, compaction
  shaiya-patch/       psNNNN.patch: build, list, apply; Ed25519 signatures
  shaiya-clientdata/  read-only client readers: archive or folder source, .wld, .dg, .smod, textures, sysmsg, Monster / NpcQuest
  shaiya-inspect/     identify a file and describe it (used by `shaiya info` and Workbench)
  shaiya-ui/          the GUI shell (egui / eframe)
  shaiya-cli/         the `shaiya` program
  shaiya-workbench/      the `shaiya-workbench` program
  shaiya-testkit/     dev-only test helpers (no game data)
  shaiya-build/       build-script helper: Windows resources, git hash, target
  shaiya-matrix-tests/  opt-in version-matrix tests over the test clients
xtask/                development tasks (dist, notices, deps-check, guard, sizes, release-notes, icons)
```

Names reserved for later tools: `shaiya-sdata`, `shaiya-sdata-editor`, `shaiya-packetlog`, `shaiya-log-viewer`, `shaiya-svmap`, `shaiya-svmap-editor`, `shaiya-updater`.

## Dependency rules

```
shaiya-core  <-  shaiya-seed
shaiya-core  <-  shaiya-archive  <-  shaiya-patch
shaiya-archive + shaiya-seed  <-  shaiya-clientdata
core + seed + archive + patch  <-  shaiya-inspect
libraries  <-  shaiya-cli
shaiya-core + eframe / egui / rfd / serde  <-  shaiya-ui  <-  the GUI programs
shaiya-testkit: dev-dependency only (std only, depends on no toolkit crate)
shaiya-build: build-dependency only
```

1. **Core libraries never depend on `egui`, `eframe`, `rfd` or `clap`, never print, never read environment variables (tests excepted), never start a process.** No crate opens a network connection.
2. **All long work takes a `&Job`** (`shaiya_core::Job`) so the CLI shows progress and the GUI can cancel.
3. **One error type**: `shaiya_core::Error` (`Io`, `Format { file, offset, what }`, `Unsupported`, `Refused`, `Cancelled`) with `ErrorKind`. `shaiya-clientdata` keeps its own `Error` (its public API is frozen) with a conversion from the core error.
4. Library crates use caret version requirements; the GUI stack is pinned exactly (egui minor versions break APIs); `Cargo.lock` is committed and CI builds with `--locked`.
5. `unsafe_code` is denied in the workspace; the only `unsafe` is the Windows console check in `shaiya-cli` (one function, with a comment).

## Safety rules

1. **No game files** in the repository, fixtures, CI artifacts or releases (`cargo xtask guard`). The tools never download anything.
2. **Never modify an input in place without a backup or an explicit output path.** In-place writes go through `shaiya_core::fs::plan_output` and `make_backup`.
3. **Atomic writes only** (`write_atomic`, `write_atomic_from`).
4. **Never execute anything from the user's files.** Launching a sibling toolkit program with a path argument is the only process start.
5. **Untrusted input:** parsers are bounds-checked (`shaiya_core::bin::Reader`), counts are checked against the remaining bytes, ZIP entries go through `enclosed_name`, decompression is capped to what the index refers to; a malformed file yields an error with file and byte offset, never a panic.
6. **Raw names stay raw.** Archive names are `Vec<u8>`; display decodes them; writes never re-encode a displayed name.
7. **No network, no telemetry** in any library or program (`cargo xtask deps-check`).
8. **Signed patches:** the signature message format in `shaiya-patch::sign` does not change without a new message version.

## Data flow

```
client folder --> shaiya_archive::Archive --(entry bytes)--> shaiya_seed::unwrap --(plaintext)--> clientdata / later schema crates
                                                                         edited plaintext --> shaiya_seed::wrap --> core::fs::write_atomic (new file)
              --> shaiya_patch::apply (patch zip) --> data.sah (backup first) + appended data.saf
CLI: clap args -> Ctx (json / quiet / progress) -> library call with a Job -> Report (text or JSON) -> exit code
GUI: event -> Tasks::spawn(work(&Job)) on a worker thread -> TaskHandle::poll each frame -> view update / error banner
```

## What later tools may assume

- New tool = new `crates/<name>` plus a `TOOLS` entry in `xtask/src/tools.rs` ([adding-a-tool.md](adding-a-tool.md)).
- Errors: `shaiya_core::{Error, ErrorKind, Result}`; the CLI maps `ErrorKind` to exit codes ([the shaiya page](../tools/shaiya.md)).
- Long work: library functions take `job: &shaiya_core::Job`; the GUI runs them through `shaiya_ui::Tasks::spawn`.
- Writing: `shaiya_core::fs::{plan_output, make_backup, write_atomic, write_atomic_from}` and the flags `--out`, `--in-place`, `--force`, `--dry-run`.
- Archives and patches: `shaiya_archive::{Archive, Index, Entry}`, `shaiya_archive::compact`, `shaiya_patch::{build, list, removals, apply, sign}`; SEED: `shaiya_seed::{unwrap, wrap, decrypt, encrypt, info, decrypt_unchecked}`; identification: `shaiya_inspect::inspect`.
- Text: `shaiya_core::text::Encoding` and `display_lossless`.
- CLI: command groups under `shaiya`; output through the `Report` trait (text plus `serde::Serialize` for `--json`).
- GUI: implement `shaiya_ui::ToolApp` ([gui-shell.md](gui-shell.md)).
- Tests: `shaiya_testkit` helpers, `SHAIYA_CLIENT`, `SHAIYA_TEST_CLIENTS`, the matrix crate with its `EXPECTED` table.
- Release: one workspace version, assets as in [releasing.md](releasing.md).
