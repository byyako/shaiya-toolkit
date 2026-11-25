# Releasing

## Versions

**One workspace version** (`[workspace.package] version` in the root `Cargo.toml`) is used by every crate and program. Tags are `vX.Y.Z`. Pre-1.0 minor versions may break library APIs; the CLI's JSON and exit codes follow [the shaiya page](../tools/shaiya.md).

## The changelog

`CHANGELOG.md` follows Keep a Changelog: `## [Unreleased]`, then `## [X.Y.Z] - YYYY-MM-DD` with subsections per tool (`### shaiya (CLI)`, `### Shaiya Toolkit`, `### Libraries`, `### Breaking` as needed). The date is the maintainer's local date on the day of the version bump commit. `cargo xtask release-notes <version>` prints the section of a version and fails when it is missing or empty.

## Making a release

1. Move `[Unreleased]` to a version section and bump the workspace version; commit.
2. **Merge to `main` before tagging.** A tag is made on the commit that is on `main`, never on a side branch, so the release and the default branch agree.
3. **The dry run must be green.** Run the release workflow as a dry run (`workflow_dispatch`, `dry_run` = true, the default) on that commit and check the artifact names, contents and checksums. Do not tag while it is red.
4. **Make the repository public** before the first release if it is not yet. Build provenance attestations are only made for public repositories, and the download links in the README and install page only work for the public.
5. **Enable private vulnerability reporting** (Settings > Code security > Private vulnerability reporting) once the repository is public; [SECURITY.md](../../SECURITY.md) tells reporters to use it, and it cannot be switched on for a private repository.
6. Tag `vX.Y.Z` and push the tag. The workflow builds every platform, runs the tests, packs the zips and creates a **draft** release with `SHA256SUMS.txt` and the notes from the changelog.
7. A **manual non-dry run** (`workflow_dispatch` with `dry_run` = false) also creates a draft release, but for the ref it was started on. If you use it instead of a tag push, dispatch it **on the tag** (choose the tag in "Use workflow from"), never on a branch, or the release is named after the branch.
8. Check the draft and publish it by hand.

`cargo xtask dist --platform windows-x86_64` builds the same zips locally for testing; **release assets come from CI only**.

## Release assets

`<name>-<version>-<platform>.zip` with `name` either `shaiya-toolkit` (the desktop apps Shaiya Toolkit and Shaiya SVMap Editor, and the `shaiya` command line: the programs `shaiya-toolkit`, `shaiya-svmap-editor` and `shaiya`) or `shaiya` (the command line only, which includes `shaiya svmap`), and `platform` in `windows-x86_64`, `linux-x86_64`, `macos-universal`; plus `SHA256SUMS.txt`. Each zip holds a folder named like the zip with the program(s), `README.txt`, `LICENSE.txt` and `THIRD-PARTY-NOTICES.txt`. On macOS each GUI is an app bundle (`io.github.byyako.<tool>`, ad-hoc signed), the CLI a plain universal binary. Zips are deterministic (sorted entries, fixed timestamps from the commit). The workflow signs nothing with keys (there are none); a repository that is public also gets a build provenance attestation.

The `README.txt` inside each zip carries the same notice as the repository README: these tools work on your own copy of the game, contain no game files and download none. In the zip that holds Shaiya SVMap Editor it also says what the editor writes (including that temporary files go to the system temp folder); the command-line zip says everything else is written only to a path you name.

## Budgets

| Item | Budget |
|---|---|
| `shaiya.exe` (command line), release, Windows x86_64 | 6 MB (measured (0.4.1 dry run) 4,744,704 bytes) |
| `shaiya-toolkit.exe` (GUI), release, Windows x86_64 | 20 MB (measured (0.4.1 dry run) 15,938,560 bytes; local builds on the 0.5.0 branch: 17,109,504 bytes before the sound player and JPG/PNG, 17,392,640 after, of which cpal and `zune-jpeg` 266,752 and `png` 16,384) |
| `shaiya-svmap-editor.exe` (GUI), release, Windows x86_64 | 20 MB (measured (0.4.1 dry run) 18,051,584 bytes) |
| `shaiya-toolkit` zip (all three programs), Windows x86_64 | 17,000,000 bytes (measured (0.4.1 dry run) 15,359,516 bytes) |
| `shaiya` zip (command line only), Windows x86_64 | 3,000,000 bytes (measured (0.4.1 dry run) 2,016,966 bytes) |
| SAH parse of a 48,000-file index | under 150 ms (release) |
| SAF handling | never loaded whole; buffers of at most 1 MiB |
| SEED decrypt or encrypt | at least 40 MB/s single thread (release) |
| SHA-256 of a 2 GB `data.saf` | at least 300 MB/s when the disk allows; cancel takes effect within 250 ms |
| CI run | under 25 minutes |

`cargo xtask sizes` checks the binary sizes (it looks in `CARGO_TARGET_DIR` when that is set, else in `target/`); `cargo xtask dist --platform windows-x86_64` checks the zip sizes. The zip budget is the measured Windows size rounded up to the next whole MB, plus 2 MB (`shaiya-toolkit` zip) or 1 MB (`shaiya` zip). A missed budget is reported with the number, not silently accepted.

## CI

`ci.yml` runs on every push and pull request: `checks` (guard, notices, deps-check, clippy) and the tests on Ubuntu and Windows; macOS runs on pushes to `main`, pull requests, tags and manual runs. The real-window smoke step on Windows is non-blocking until it has passed on three consecutive runs. `docs.yml` builds the site on pull requests and publishes it to GitHub Pages from `main` and from `v*` tags.

One-time owner steps for Pages: Settings > Pages > Source = GitHub Actions; Settings > Environments > `github-pages` > add a deployment tag rule `v*` (`main` stays). The Pages site is public even if the repository is private, and on a private repository Pages needs a paid plan, so the deploys fail (red) until the repository is public and Pages is enabled. See [docs-site.md](docs-site.md#publishing).
