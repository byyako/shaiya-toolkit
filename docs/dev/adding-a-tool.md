# Adding a tool

A checklist. A tool is a library plus a thin program (or a command group in `shaiya`).

1. **Crate.** `crates/<name>` for the library (named `shaiya-<thing>`), depending only on the core crates it needs ([architecture.md](architecture.md)). Take `version`, `edition`, `license`, `authors`, `repository`, `publish` and `[lints]` from the workspace. Library code takes a `&shaiya_core::Job` for long work, returns `shaiya_core::Result` (the exceptions of [architecture.md](architecture.md), rule 3, apply), never prints and never writes except through `shaiya_core::fs`.
2. **Program.** For a CLI tool: `crates/shaiya-cli/src/cmd/<group>.rs`, one enum variant in `main.rs`, a `Report` type with text and `serde::Serialize` output, exit codes from `ErrorKind`. For a GUI tool: `crates/<name>` with a library implementing `shaiya_ui::ToolApp`, a `main.rs` calling `shaiya_ui::run` (standalone programs such as Shaiya SVMap Editor use their own shell and call neither), a `build.rs` calling `shaiya_build` (Windows resources, git hash), `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`.
3. **`TOOLS` entry.** Add the program to `xtask/src/tools.rs` (package, binary, asset name, title, `gui`, macOS bundle id). `dist`, `sizes`, `notices` and `deps-check` pick it up from there. For a GUI program also add it to `GUI_PROGRAMS` in `crates/shaiya-ui/src/lib.rs` (the Tools menu lists it; the xtask test `gui_programs_match_tools` fails when the two lists differ).
4. **Icon.** Add an entry to `xtask/src/icons.rs`, run `cargo xtask icons`, commit `assets/icons/`. The window icon is `<id>-64.rgba`; the build script uses `<id>.ico`.
5. **Tests.** Unit tests, round trips on synthetic data (build inputs with `shaiya-testkit`), robustness tests for every parser, CLI tests for every command, harness tests for every GUI ([testing.md](testing.md)).
6. **Matrix rows.** Add the capability to `EXPECTED` in `crates/shaiya-matrix-tests` and to [../version-matrix.md](../version-matrix.md) in the same commit.
7. **CHANGELOG.** An entry under `[Unreleased]` in the tool's subsection.
8. **README tables.** The tools table and, for a new program, the download table in `README.md`.
9. **Docs page.** `docs/tools/<tool>.md` and a row in `docs/README.md`. Format pages go under `docs/formats/`. No local paths, no addresses, no names other than the author's.
10. **Checks.** `cargo build --workspace --locked` with zero warnings, `cargo test --workspace --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo xtask guard`, `cargo xtask notices --check`, `cargo xtask deps-check`. Every new dependency is named, with its reason, in the pull request.
