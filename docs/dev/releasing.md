# Releasing

## Versions

**One workspace version** (`[workspace.package] version` in the root `Cargo.toml`) is used by every crate and program. Tags are `vX.Y.Z`. Pre-1.0 minor versions may break library APIs; the CLI's JSON and exit codes follow [the shaiya page](../tools/shaiya.md).

## The changelog

`CHANGELOG.md` follows Keep a Changelog: `## [Unreleased]`, then `## [X.Y.Z] - YYYY-MM-DD` with subsections per tool (`### shaiya (CLI)`, `### Workbench`, `### Libraries`, `### Breaking` as needed). `cargo xtask release-notes <version>` prints the section of a version and fails when it is missing or empty.

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

`<tool>-<version>-<platform>.zip` with `tool` in `shaiya`, `shaiya-workbench` (later the other programs) and `shaiya-toolkit` (everything), and `platform` in `windows-x86_64`, `linux-x86_64`, `macos-universal`; plus `SHA256SUMS.txt`. Each zip holds a folder named like the zip with the program(s), `README.txt`, `LICENSE.txt` and `THIRD-PARTY-NOTICES.txt`. On macOS each GUI is an app bundle (`io.github.byyako.<tool>`, ad-hoc signed), the CLI a plain universal binary. Zips are deterministic (sorted entries, fixed timestamps from the commit). The workflow signs nothing with keys (there are none); a repository that is public also gets a build provenance attestation.

The `README.txt` inside each zip carries the same sentence as the repository README: these tools work on your own copy of the game, contain no game files and download none.

## Budgets

| Item | Budget |
|---|---|
| `shaiya` CLI, release, Windows x86_64 | 6 MB |
| each GUI program, release, Windows x86_64 | 20 MB |
| zip per GUI tool | 9 MB |
| SAH parse of a 48,000-file index | under 150 ms (release) |
| SAF handling | never loaded whole; buffers of at most 1 MiB |
| SEED decrypt or encrypt | at least 40 MB/s single thread (release) |
| SHA-256 of a 2 GB `data.saf` | at least 300 MB/s when the disk allows; cancel takes effect within 250 ms |
| CI run | under 25 minutes |

`cargo xtask sizes` checks the binary sizes. A missed budget is reported with the number, not silently accepted.

## CI

`ci.yml` runs on every push and pull request: `checks` (guard, notices, deps-check, clippy) and the tests on Ubuntu and Windows; macOS runs on pushes to `main`, pull requests, tags and manual runs. The real-window smoke step on Windows is non-blocking until it has passed on three consecutive runs. `docs.yml` builds the site on pull requests and publishes it to GitHub Pages from `main` and from `v*` tags.
