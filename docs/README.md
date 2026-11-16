# Documentation

| Page | What it covers |
|---|---|
| [install.md](install.md) | Downloads, checking them against `SHA256SUMS.txt`, platform notes |
| [version-matrix.md](version-matrix.md) | What parses on which client |
| [formats/archive.md](formats/archive.md) | `data.sah` / `data.saf`: layout, names, hashing, free list, versions 0 and 1 |
| [formats/patch.md](formats/patch.md) | `psNNNN.patch`: ZIP layout, `remove.txt`, loose files, signatures |
| [formats/sdata-wrapper.md](formats/sdata-wrapper.md) | The SEED wrapper of SData files, CRC, plain files |
| [tools/shaiya.md](tools/shaiya.md) | The `shaiya` command line: flags, conventions, exit codes, JSON, `info`, `version` |
| [tools/studio.md](tools/studio.md) | Shaiya Archive & Patch Studio (the file inspector) |
| [dev/architecture.md](dev/architecture.md) | Crates, dependency rules, data flow, what later tools may assume |
| [dev/gui-shell.md](dev/gui-shell.md) | The `shaiya-ui` shell: writing a tool on it |
| [dev/testing.md](dev/testing.md) | Test levels, the testkit, opt-in tests on your own client, the matrix |
| [dev/releasing.md](dev/releasing.md) | Versions, the changelog, tags, the release workflow |
| [dev/adding-a-tool.md](dev/adding-a-tool.md) | Checklist for a new tool |

These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any.
