# Security policy

## Reporting a vulnerability

Please report security problems **privately** through GitHub: open the repository's Security tab and choose **Report a vulnerability** (a private advisory). Do not open a public issue for it. You will get an answer as soon as the maintainer can read it; please allow a few days.

## What is in scope

- The patch signature checks (`shaiya-patch::sign`): anything that lets an altered, renamed or foreign-key patch pass.
- Path handling in patches and in extraction: anything that writes outside the chosen folder (`..`, absolute paths, links).
- Crashes or hangs on malformed files (a malformed file should give an error with a byte offset, never a panic, an unbounded allocation or a loop).
- Anything that writes a file without an explicit output path or the journal and backup the documentation promises, or that leaves a half-written target. (Known and documented: `--no-journal` keeps only the previous index as `data.sah.bak`, and `archive compact --in-place` cannot be undone.)

Not in scope: problems that need an attacker to already control your computer, and the content of game files themselves.

## Supported versions

Only the latest release gets fixes.

## What the programs do not do

The libraries and programs open no network connections, send no telemetry and never download anything. They start other programs in two cases only, both when you ask for it in Shaiya Toolkit: a sibling toolkit program you launch from the Tools menu, and your operating system's own handler to open a folder or file in the file manager (Show folder, Help > Open log file, Open data folder) or a web page in your default browser (Help > Support the project, which is only a link to the author's donation page). Nothing is opened automatically, and nothing is run from the files you work on.
