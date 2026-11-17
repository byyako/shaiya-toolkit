# Editing these docs

This site is built from the `docs/` folder plus three files from the repository root: `CHANGELOG.md`, `CONTRIBUTING.md` and `SECURITY.md`. A hook in `.github/docs-site/hooks.py` adds those three as pages. Nothing is copied, so the root files stay where they are. `docs/README.md` is the home page; do not add an `index.md`.

## Links

Link to other pages the way GitHub wants it: a relative path to the `.md` file, with an optional `#anchor`. The hook rewrites each link for the site. A link to a file in `docs/` or to one of the three root pages becomes a site link. A link to any other file, such as `LICENSE` or a source file, goes to that file on GitHub. The build checks every link and anchor and fails on a broken one.

## New pages

Add every new page to `nav` in `mkdocs.yml`. A page that is not in `nav` fails the strict build, so it cannot be forgotten.

## Screenshots

Screenshots live in `docs/assets/screenshots/`. Put them in with normal Markdown image syntax and a relative path, so they show on GitHub too. While a PNG is not committed, the hook leaves the image line out and logs it at INFO level, and the build still passes.

- PNG, at most 1 MiB (`cargo xtask guard` enforces this); aim for 300 KB or less, about 1600 px wide.
- No game art: no textures, models or UI images from game files. Use a synthetic archive, or show file names only.
- No personal paths or user names in the window.

## Preview locally

Make a virtual environment outside the repository, then from the repository root:

```
python -m venv <somewhere outside the repo>
<venv>/Scripts/python -m pip install -r .github/docs-site/requirements.txt
<venv>/Scripts/python -m mkdocs serve
```

On Linux and macOS the venv programs are in `bin` instead of `Scripts`: `<venv>/bin/python -m pip ...` and `<venv>/bin/python -m mkdocs serve`. Before a pull request, run the same checks as CI:

```
<venv>/Scripts/python -m mkdocs build --strict
<venv>/Scripts/python -I .github/docs-site/check_site.py site
```

The theme prints a notice about its end of life. Set `NO_MKDOCS_2_WARNING=1` to silence it. `site/` is ignored by git.

## Publishing

The workflow `docs.yml` publishes the site. The repository owner does these steps once:

1. Settings > Pages > Source: choose **GitHub Actions**.
2. Settings > Environments > `github-pages`: add a deployment tag rule `v*` (the `main` branch rule stays), so releases can deploy.

A Pages site is public even when the repository is private. On a private repository, Pages needs a paid plan, so the deploy job fails (red) until the repository is public and Pages is enabled.

## Updating the pins

`requirements.in` lists the top-level pins and `requirements.txt` is compiled from it. The command is in the header of `requirements.txt`. Use `--exclude-newer` with a date 14 days back, so no release is newer than two weeks.

## What the site leaves out

- No analytics, no consent banner and no external fonts (`font: false`). The built pages request nothing from other hosts, and `check_site.py` fails if one does.
- No `repo_url` or `edit_uri`: the theme's script would call the GitHub API from every visitor's browser. The GitHub link is an icon link in the footer.

## Toolchain end of life

Material for MkDocs reaches end of life on 2027-05-05. The output is static and the versions are pinned, so the site keeps building. Plan a move to its successor before then; the `hooks.py` file may need porting.
