# Documentation

| Page | What it covers |
|---|---|
| [getting-started.md](getting-started.md) | Five minutes: inspect, extract, build and apply a patch on a copy, undo it |
| [faq.md](faq.md) | Exit code 4 (files in use), SmartScreen, unsupported clients, exit codes, backups |
| [install.md](install.md) | Downloads, checking them against `SHA256SUMS.txt`, platform notes, where data lives |
| [guides/mod-files.md](guides/mod-files.md) | Modders: browse, extract, edit, add with a backup, make a patch, roll back |
| [guides/make-a-patch.md](guides/make-a-patch.md) | Operators: build, sign, test on a copy, publish `psNNNN.patch` |
| [guides/sdata-files.md](guides/sdata-files.md) | Decrypt and encrypt SData around an editor, `--like`, unencrypted clients |
| [guides/undo-and-recovery.md](guides/undo-and-recovery.md) | Journals, rollback, what blocks it, the pending marker, swap back |
| [version-matrix.md](version-matrix.md) | What parses on which client |
| [formats/archive.md](formats/archive.md) | `data.sah` / `data.saf`: layout, names, hashing, shadowing, free list, versions 0 and 1, offsets past 4 GiB, compaction |
| [formats/patch.md](formats/patch.md) | `psNNNN.patch`: ZIP layout, `remove.txt`, loose files and their name rules, deterministic builds, journaled apply, signatures |
| [formats/sdata-wrapper.md](formats/sdata-wrapper.md) | The SEED wrapper of SData files, CRC, plain files |
| [tools/shaiya.md](tools/shaiya.md) | The `shaiya` command line: every command (`info`, `archive`, `patch`, `sdata`, `clients`, `version`), flags, exit codes, JSON |
| [tools/workbench.md](tools/workbench.md) | Shaiya Workbench: the six tabs, client folders, pending changes, prompts, keyboard |
| [dev/architecture.md](dev/architecture.md) | Crates, dependency rules, data flow, what later tools may assume |
| [dev/gui-shell.md](dev/gui-shell.md) | The `shaiya-ui` shell: writing a tool on it |
| [dev/testing.md](dev/testing.md) | Test levels, the testkit, opt-in tests on your own client, the matrix |
| [dev/releasing.md](dev/releasing.md) | Versions, the changelog, tags, the release workflow |
| [dev/adding-a-tool.md](dev/adding-a-tool.md) | Checklist for a new tool |

These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. Changes to a client folder are recorded in a journal so they can usually be rolled back, but not changes made with `--no-journal` or `archive compact --in-place`, which keep at most the previous index (`data.sah.bak`). Everything else is written only to a path you name.

Support the project: https://buymeacoffee.com/tylernol
