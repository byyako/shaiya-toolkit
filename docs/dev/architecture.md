# Architecture

Every tool is a thin command-line or GUI layer over one shared, tested Rust core.

## Crates

```
crates/
  shaiya-core/        errors, jobs and progress, safe writes, binary reader, text encodings, hashing, UTC time,
                      glob patterns and filters, byte sources (folder, archive), saved client folders and the data folder
  shaiya-seed/        SEED cipher, the SData wrapper, and batch decrypt / encrypt over byte sources
  shaiya-archive/     data.sah / data.saf: index, reading, layout analysis and check, extract, edit (in place and new pair),
                      journals and rollback, client folder recovery, compaction, diff, pack, swap
  shaiya-patch/       psNNNN.patch: build (streaming, deterministic), list, verify, plan, apply with journals; Ed25519 signatures
  shaiya-clientdata/  read-only client readers: archive or folder source, .wld, .dg, .smod, textures, sysmsg, Monster / NpcQuest
  shaiya-svmap/       the .svmap server map format: model, reader, writer, validation, bit grid, JSON (no GUI, no client data)
  shaiya-svmap-tools/ the SVMap editor's engine without a GUI: documents with undo, client pairing, checks, spawn report, bulk planner,
                      Map.ini, new map, install; `batch` holds the entry points of `shaiya svmap`
  shaiya-inspect/     identify a file and describe it (used by `shaiya info` and Shaiya Toolkit)
  shaiya-ui/          the GUI shell (egui / eframe)
  shaiya-cli/         the `shaiya` program (info, archive, patch, sdata, clients, version)
  shaiya-toolkit/   the `shaiya-toolkit` program (Archive, Patch builder, Apply & history, Sign & verify, SData, Inspect)
  shaiya-svmap-editor/ the `shaiya-svmap-editor` program (Shaiya SVMap Editor: 2D and 3D map views, checks, paired client edits, Map.ini,
                      new maps, patch export, install into a test client); borrows Paths and the theme base from shaiya-ui
  shaiya-testkit/     dev-only test helpers (no game data)
  shaiya-build/       build-script helper: Windows resources, git hash, target
  shaiya-matrix-tests/  opt-in version-matrix tests over the test clients
xtask/                development tasks (dist, notices, deps-check, guard, sizes, release-notes, icons)
```

Names reserved for later tools: `shaiya-sdata`, `shaiya-sdata-editor`, `shaiya-packetlog`, `shaiya-log-viewer`, `shaiya-updater`.

### Modules worth knowing

| Crate | Module | What it holds |
|---|---|---|
| `shaiya-core` | `glob` | The one pattern syntax of every command and tab; `Filter` (include, exclude, size range); `parse_list` |
| `shaiya-core` | `source` | `ByteSource`, `FolderSource` (naming and collision refusal), shared by SData batch work and the GUI |
| `shaiya-core` | `config` | `data_root` (where settings live), `shared.json` with `update_clients` (read-modify-write that keeps unknown keys), `validate_client_name` |
| `shaiya-core` | `fs` | `write_atomic`, `plan_output`, `make_backup`, `portable_name`, `Existing` (refuse, skip, overwrite) |
| `shaiya-archive` | `layout`, `check` | Gaps, overlaps, shared ranges, out-of-bounds entries; every `check` finding code |
| `shaiya-archive` | `edit` | `EditPlan`; `apply_in_place` (journaled, crash safe); `write_new` (a compacted new pair); `loose_name_problem` |
| `shaiya-archive` | `journal`, `client` | Journals, the pending marker, rollback and its blocking reasons; `ClientFiles`, the in-use check, `recover` |
| `shaiya-archive` | `compact`, `swap`, `pack`, `diff`, `extract` | One module each; compaction and swap have their own crash markers |
| `shaiya-patch` | `sign` | The signature message and key files; the format does not change |
| `shaiya-seed` | `batch` | Check, decrypt and encrypt over many files, with `--like` |
| `shaiya-svmap-tools` | `batch`, `episode`, `keys`, `stamp` | The `shaiya svmap` entry points and their report types; notes about a client that is not US EP4.5; the settings enums and path key; the fixed UTC stamp of backup and record names |
| `shaiya-toolkit` | `archive_view`, `patch_builder`, `apply_tab`, `sign_tab`, `sdata_tab`, `inspect`, `clients` | One module per tab and the client-folder dialogs |

## Dependency rules

```
shaiya-core  <-  shaiya-seed
shaiya-core  <-  shaiya-archive  <-  shaiya-patch
shaiya-archive + shaiya-seed  <-  shaiya-clientdata
serde + serde_json + base64  <-  shaiya-svmap
core + archive + patch + clientdata + svmap  <-  shaiya-svmap-tools
core + seed + archive + patch  <-  shaiya-inspect
libraries (now including svmap and svmap-tools)  <-  shaiya-cli
shaiya-core + eframe / egui / rfd / serde  <-  shaiya-ui  <-  the GUI programs
shaiya-ui (Paths/Env, theme) + svmap-tools + eframe / egui / egui-wgpu / wgpu / bytemuck / rfd  <-  shaiya-svmap-editor
shaiya-testkit: dev-dependency only (std only, depends on no toolkit crate)
shaiya-build: build-dependency only
```

1. **Core libraries never depend on `egui`, `eframe`, `rfd` or `clap`, never print, never read environment variables (tests excepted), never start a process.** No crate opens a network connection.
2. **All long work takes a `&Job`** (`shaiya_core::Job`) so the CLI shows progress and the GUI can cancel.
3. **One error type**: `shaiya_core::Error` (`Io`, `Format { file, offset, what }`, `Unsupported`, `Refused`, `Cancelled`) with `ErrorKind`. `shaiya-clientdata` keeps its own `Error` (its public API is kept stable; 0.2.0 adds `wld_raw`, `area_txt` and `Npc.gates`) with a conversion from the core error. Two more exceptions: `shaiya-svmap` keeps its `ReadError` and `JsonError` (it depends on no other toolkit crate), and `shaiya-svmap-tools` keeps `Result<_, String>` inside the modules moved from the editor; only its `batch` module (the entry points of `shaiya svmap`) returns `shaiya_core::Result` with the kinds the exit codes need.
4. Library crates use caret version requirements; the GUI stack is pinned exactly (egui minor versions break APIs); `Cargo.lock` is committed and CI builds with `--locked`.
5. `unsafe_code` is denied in the workspace; the only `unsafe` is two small Windows FFI calls, each in one function with a comment and a SAFETY note: the console check in `shaiya-cli` (`GetConsoleProcessList`) and the folder identity read in `shaiya-svmap-tools` (`GetFileInformationByHandle`, used to tell two names of one folder apart).

## Changing a client folder safely

Every change to a client folder (archive edits, patch apply, rollback, compaction, swap) follows the same rules, implemented once in `shaiya-archive` and reused by the CLI and Shaiya Toolkit. The user-facing description is [undo and recovery](../guides/undo-and-recovery.md); in short:

1. **Recover first.** `client::recover` completes or discards an interrupted compaction, an interrupted swap (`data.swap-journal`) and an unfinished change (`data.toolkit-pending`, the pending marker) before anything else.
2. **Check that nothing else uses the files.** On Windows, open `data.sah`, `data.saf` (and `game.exe`) for write with sharing denied; a sharing violation is `Refused` with a message that says close the game. Other systems cannot tell.
3. **Validate everything before writing.** Names (code page 949, loose-file rules), duplicates, missing paths and the signature are checked and all problems reported together.
4. **Journal and marker.** Create `shaiya-toolkit-backups/<UTC stamp>-<label>/` with a copy of the old index and the hashes before and after, then write the marker.
5. **Append** the new contents to `data.saf` and sync. Nothing already in `data.saf` is overwritten. On error or cancel, truncate back to the old length.
6. **Replace the index** atomically (the last step that can fail without a way back).
7. **Loose files** are staged next to their targets, the originals saved into the journal, then renamed into place.
8. **Commit:** mark the journal applied and remove the marker.

`journal::rollback` is the inverse and is itself marker-protected; it refuses when `data.sah` is not the index the change wrote (compaction, swap, another tool), when a newer change exists, or when a loose file changed (unless forced). Compaction and swap deliberately end rollback of earlier journals. `--no-journal` skips steps 4 and 7's backups; the previous index is still copied to `data.sah.bak`. `compact --in-place` replaces `data.saf` outright and keeps only that index copy.

## Safety rules

1. **No game files** in the repository, fixtures, CI artifacts or releases (`cargo xtask guard`). The tools never download anything.
2. **Never modify an input in place without a backup or an explicit output path.** The exceptions are named: journaled client changes, and `--no-journal` / `compact --in-place` as described above, which the command line warns about. Plain output goes through `shaiya_core::fs::plan_output` and `make_backup`.
3. **Atomic writes only** (`write_atomic`, `write_atomic_from`).
4. **Never execute anything from the user's files.** Process starts are limited to two kinds, both on a user action: launching a sibling toolkit program with a path argument, and asking the operating system to open a folder, a log file or a web page in the file manager or default browser (`ctx.open_url`: "Show folder", Help > Open log file, Help > Open data folder, Help > Support the project). No program fetches or sends anything itself.
5. **Untrusted input:** parsers are bounds-checked (`shaiya_core::bin::Reader`), counts are checked against the remaining bytes, ZIP entries go through `enclosed_name`, decompression is capped to what the index refers to; a malformed file yields an error with file and byte offset, never a panic.
6. **Raw names stay raw.** Archive names are `Vec<u8>`; display decodes them; writes never re-encode a displayed name.
7. **No network, no telemetry** in any library or program (`cargo xtask deps-check`).
8. **Signed patches:** the signature message format in `shaiya-patch::sign` does not change without a new message version.
9. **Keys** are never overwritten, never stored by Shaiya Toolkit unless asked (the path only), and never printed except the public key.

## Data flow

```
client folder --> shaiya_archive::Archive --(entry bytes)--> shaiya_seed::unwrap --(plaintext)--> clientdata / later schema crates
                                                                         edited plaintext --> shaiya_seed::wrap --> core::fs::write_atomic (new file)
              --> shaiya_patch::apply_with (patch zip) --> journal + appended data.saf + data.sah (via shaiya_archive::edit)
CLI: clap args -> Ctx (json / quiet / progress) -> library call with a Job -> Report (text or JSON) -> exit code
GUI: event -> Tasks::spawn(work(&Job)) on a worker thread -> TaskHandle::poll each frame -> view update / error banner
```

## What later tools may assume

- New tool = new `crates/<name>` plus a `TOOLS` entry in `xtask/src/tools.rs` ([adding-a-tool.md](adding-a-tool.md)).
- Errors: `shaiya_core::{Error, ErrorKind, Result}`; the CLI maps `ErrorKind` to exit codes ([the shaiya page](../tools/shaiya.md)).
- Long work: library functions take `job: &shaiya_core::Job`; the GUI runs them through `shaiya_ui::Tasks::spawn`.
- Writing: `shaiya_core::fs::{plan_output, make_backup, write_atomic, write_atomic_from}`; changes to a client folder go through `shaiya_archive::edit::apply_in_place` and `shaiya_patch::apply_with`, never around them.
- Archives and patches: `shaiya_archive::{Archive, Index, Entry}` and its modules, `shaiya_patch::{build_with, list, verify, plan_seq, apply_with, sign}`; SEED: `shaiya_seed::{unwrap, wrap, decrypt, encrypt, info, decrypt_unchecked, batch}`; identification: `shaiya_inspect::inspect`.
- Text: `shaiya_core::text::Encoding` and `display_lossless`.
- CLI: command groups under `shaiya`; output through the `Report` trait (text plus `serde::Serialize` for `--json`).
- GUI: implement `shaiya_ui::ToolApp` ([gui-shell.md](gui-shell.md)).
- Tests: `shaiya_testkit` helpers, `SHAIYA_CLIENT`, `SHAIYA_SCRATCH`, `SHAIYA_TEST_CLIENTS`, the matrix crate with its `EXPECTED` table ([testing.md](testing.md)).
- Release: one workspace version, assets as in [releasing.md](releasing.md).
