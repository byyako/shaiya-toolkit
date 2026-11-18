# Contributing

Thanks for helping. The toolkit is a Rust workspace; every tool is a thin command-line or GUI layer over a shared, tested library. Read [docs/dev/architecture.md](docs/dev/architecture.md) first.

## Build and test

```
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The build must have **zero warnings**; CI runs with `RUSTFLAGS=-D warnings`. Other checks CI runs (you can run them too):

```
cargo xtask guard         # no game files among the tracked files
cargo xtask notices --check
cargo xtask deps-check    # licence allowlist, no network crates
```

### Tests on your own client (opt-in)

No game data lives in the repository. Tests that need real files skip with a message unless you set:

- `SHAIYA_CLIENT`: a client folder holding `data.sah` and `data.saf` (and `game.exe`). Read only.
- `SHAIYA_SCRATCH`: a folder outside any repository with a few GB free. With `SHAIYA_CLIENT`, the tests copy the client there and apply, roll back, compact and swap on the copy; the original is never written.
- `SHAIYA_TEST_CLIENTS`: the version-matrix folder, see [docs/dev/testing.md](docs/dev/testing.md).

```
SHAIYA_CLIENT=/path/to/client cargo test --workspace -- --nocapture
SHAIYA_TEST_CLIENTS=/path/to/matrix SHAIYA_CLIENT=/path/to/client cargo test -p shaiya-matrix-tests -- --nocapture --test-threads=1
```

## Rules

- **Zero warnings, clippy clean.** A targeted `#[allow]` needs a comment saying why.
- **The CLI and the GUIs stay thin over a library.** Format knowledge, parsing and writing live in the library crates, never in a program.
- **Every new dependency is named, with its reason, in the pull request.** Library crates use caret requirements; the GUI stack is pinned exactly. No network code anywhere; `cargo xtask deps-check` fails on HTTP and TLS crates.
- **No game data** in the repository, in tests, fixtures, CI artifacts or releases: no `data.sah`, `data.saf`, SData taken from a client, `game.exe`, or `.patch` made from client data, and no records or names copied out of game data. Tests build their inputs with `shaiya-testkit`.
- **No code from sources without a licence**, such as decompilation projects. Cite the licence of anything adapted.
- **Tests for every format change**, and the version-matrix rows updated with the code: when a client's result changes, edit `EXPECTED` (and `NUMBERS`) in `crates/shaiya-matrix-tests/src/lib.rs` and the table in `docs/version-matrix.md` **in the same commit** as the code that changed it. The matrix tests fail when a cell differs, an improvement included.
- **Writes are safe:** every write goes through `shaiya_core::fs` (atomic, with a backup or an explicit output path). Changes to a client folder go through the journaled procedure in `shaiya-archive` ([docs/dev/architecture.md](docs/dev/architecture.md#changing-a-client-folder-safely)), never around it.
- **Docs travel with the code.** A new or changed command, flag, exit code, JSON key, confirmation or shortcut updates [docs/tools/shaiya.md](docs/tools/shaiya.md) or [docs/tools/toolkit.md](docs/tools/toolkit.md) and the CHANGELOG in the same pull request. Examples use neutral paths (`C:\Games\Shaiya`, `D:\mods`).

## Where things go

- A new tool: follow [docs/dev/adding-a-tool.md](docs/dev/adding-a-tool.md) (crate, `TOOLS` entry, icon, CHANGELOG, README tables, matrix rows, docs page).
- GUI work: [docs/dev/gui-shell.md](docs/dev/gui-shell.md).
- Releases: [docs/dev/releasing.md](docs/dev/releasing.md).

## Commits and pull requests

A summary line, then a body that says **why**. Keep a commit to one change. The pull request template asks what changed, why, how it is tested, which dependencies were added, and which docs were updated.

Report security problems privately, see [SECURITY.md](SECURITY.md).
