# Shaiya Toolkit

Command-line and desktop tools for Shaiya game data, built on one tested Rust core. **Shaiya Toolkit** is the desktop app (`shaiya-toolkit`); **Shaiya SVMap Editor** (`shaiya-svmap-editor`) is the desktop editor for server map files; **`shaiya`** is the command line. Browse, extract and edit a client's `data.sah`/`data.saf`, build, sign, apply and roll back `psNNNN.patch` files, convert SData files, identify any client file, and edit, check and make server maps (`.svmap`). Changes the toolkit's other tools make to a client folder are journaled so they can usually be rolled back.

> **These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. Changes the toolkit's other tools make to a client folder are recorded in a journal so they can usually be rolled back, but not changes made with --no-journal or archive compact --in-place, which keep at most the previous index (data.sah.bak). Shaiya SVMap Editor saves server map files, Map.ini and MapWar.ini where you open them (keeping a .bak of the first version), writes client files to a staging folder you pick, writes patches and folder exports only where you say, and changes data.sah/data.saf only of a test client you pick with Install, which keeps its own copy of the index there (no journal) so Uninstall can restore it; Copy loose adds new files to that test client. It never writes the client it reads from, and never edits game.exe or a DLL. Everything else is written only to a path you name, apart from temporary files in the system temp folder.**

## Download

Each release carries these files:

| Download | Windows | Linux | macOS |
|---|---|---|---|
| Shaiya Toolkit (desktop apps Shaiya Toolkit and Shaiya SVMap Editor, includes `shaiya`) | `shaiya-toolkit-<version>-windows-x86_64.zip` | `shaiya-toolkit-<version>-linux-x86_64.zip` | `shaiya-toolkit-<version>-macos-universal.zip` |
| `shaiya` (command line) | `shaiya-<version>-windows-x86_64.zip` | `shaiya-<version>-linux-x86_64.zip` | `shaiya-<version>-macos-universal.zip` |

All downloads are on the [releases page](https://github.com/byyako/shaiya-toolkit/releases). Every release has a `SHA256SUMS.txt`; [install.md](install.md#check-the-download) shows how to check a download against it.

## Which clients work

US, RU, FR, CN, Phoenix, KR episode 8 and Fawkes v68. What works on which client is in [version-matrix.md](version-matrix.md). Shaiya SVMap Editor is verified on US EP4.5 only; the same page has its assumptions.

## Start here

- [getting-started.md](getting-started.md): five minutes from inspecting a file to building a patch on a copy and undoing it.
- Formats: [overview of every format page](formats/README.md).
- Guides: [modding files](guides/mod-files.md), [making patches](guides/make-a-patch.md), [SData files](guides/sdata-files.md), [undo and recovery](guides/undo-and-recovery.md), [client health and clean-up](guides/client-health.md).
- [faq.md](faq.md): the common errors.

## All pages

| Page | What it covers |
|---|---|
| [getting-started.md](getting-started.md) | Five minutes: inspect, extract, build and apply a patch on a copy, undo it |
| [faq.md](faq.md) | Exit code 4 (files in use), SmartScreen, unsupported clients, exit codes, backups, the SVMap Editor's episodes, read-only maps, unsigned patches |
| [install.md](install.md) | Downloads, checking them against `SHA256SUMS.txt`, platform notes, where data lives |
| [guides/mod-files.md](guides/mod-files.md) | Modders: browse, extract, edit, add with a backup, make a patch, roll back |
| [guides/make-a-patch.md](guides/make-a-patch.md) | Operators: build, sign, test on a copy, publish `psNNNN.patch` |
| [guides/sdata-files.md](guides/sdata-files.md) | Decrypt and encrypt SData around an editor, `--like`, unencrypted clients |
| [guides/undo-and-recovery.md](guides/undo-and-recovery.md) | Journals, rollback, what blocks it, the pending marker, swap back |
| [guides/client-health.md](guides/client-health.md) | What Health reports about a client folder, the leftovers, moving them aside and putting them back |
| [version-matrix.md](version-matrix.md) | What parses on which client |
| [formats/README.md](formats/README.md) | Overview of every format page: what reads and writes it, where it was verified, and the extensions nobody has documented yet |
| [formats/archive.md](formats/archive.md) | `data.sah` / `data.saf`: layout, names, hashing, shadowing, free list, versions 0 and 1, offsets past 4 GiB, compaction |
| [formats/patch.md](formats/patch.md) | `psNNNN.patch`: ZIP layout, `remove.txt`, loose files and their name rules, deterministic builds, journaled apply, signatures |
| [formats/version-ini.md](formats/version-ini.md) | `Version.ini` (client) and `UpdateVersion.ini` (patch server): keys, folder layout, what the original updater does, what `patch publish` writes |
| [formats/sdata-wrapper.md](formats/sdata-wrapper.md) | The SEED wrapper of SData files, CRC, plain files |
| [formats/svmap.md](formats/svmap.md) | SVMap (`.svmap`): size, blocked bit grid, sections A and B, monster areas, NPCs, portals, bind areas, regions, and the JSON form |
| [formats/wld.md](formats/wld.md) | World (`.wld`): terrain, placements, sounds, portals, areas, zone groups, the two counts that must be exact |
| [formats/area-txt.md](formats/area-txt.md) | World area names (`world/<id>.txt`): UTF-16 lines, gaps, line endings kept |
| [formats/map-ini.md](formats/map-ini.md) | `Map.ini` and `MapWar.ini`: zones, `MapCount`, war map ids, line-preserving edits |
| [formats/obelisk-ini.md](formats/obelisk-ini.md) | `Obelisk.ini`: zones and obelisk positions |
| [formats/npcquest.md](formats/npcquest.md) | `NpcQuest.SData`: the 13 NPC sections, gatekeeper gates, the link table |
| [formats/monster.md](formats/monster.md) | `Monster.SData`: the record layout and the monster id |
| [formats/item-sdata.md](formats/item-sdata.md) | `Item.SData`: the item table, two layout variants, where each was verified |
| [formats/skill-sdata.md](formats/skill-sdata.md) | `Skill.SData`: the skill table, three layout variants, where each was verified |
| [formats/dg.md](formats/dg.md) | Dungeon floors (`.dg`): the octree layout and the map size rule |
| [formats/smod.md](formats/smod.md) | Models (`.smod`): modules, vertices, faces, collision, orientation |
| [formats/textures.md](formats/textures.md) | Textures: DDS and TGA as read; field chunk textures and the minimap the editor writes |
| [formats/sysmsg.md](formats/sysmsg.md) | `sysmsg-uni.txt` and the table of map names |
| [formats/tacticszone-zon.md](formats/tacticszone-zon.md) | `TacticsZone.zon`: header, areas with map id and type |
| [formats/mobs-psv.md](formats/mobs-psv.md) | Afterglow's `mobs.psv`: the monster id column |
| [formats/svmap-editor-files.md](formats/svmap-editor-files.md) | The files the editor defines: CSV export, install record, index backups, new-map note file, UTC stamp |
| [formats/toolkit-files.md](formats/toolkit-files.md) | The toolkit's own files in a client folder: journals, the pending marker, `data.sah.bak`, compaction and swap files, temporary files, cleanup folders |
| [tools/shaiya.md](tools/shaiya.md) | The `shaiya` command line: every command (`info`, `archive`, `patch`, `sdata`, `svmap`, `clients`, `version`), flags, exit codes, JSON |
| [tools/svmap-editor.md](tools/svmap-editor.md) | Shaiya SVMap Editor: the user guide (also the editor's Help), what it writes, verified episodes, workflows, rules, shortcuts |
| [tools/toolkit.md](tools/toolkit.md) | Shaiya Toolkit: the six tabs, client folders, pending changes, prompts, keyboard |
| [dev/architecture.md](dev/architecture.md) | Crates, dependency rules, data flow, what later tools may assume |
| [dev/gui-shell.md](dev/gui-shell.md) | The `shaiya-ui` shell: writing a tool on it |
| [dev/testing.md](dev/testing.md) | Test levels, the testkit, opt-in tests on your own client, the matrix |
| [dev/releasing.md](dev/releasing.md) | Versions, the changelog, tags, the release workflow |
| [dev/adding-a-tool.md](dev/adding-a-tool.md) | Checklist for a new tool |
| [dev/docs-site.md](dev/docs-site.md) | Editing these docs: links, new pages, screenshots, previewing the site, publishing to GitHub Pages |
| [Contributing](../CONTRIBUTING.md) | How to propose a change |
| [Security](../SECURITY.md) | Reporting a vulnerability |
| [Changelog](../CHANGELOG.md) | What changed in each version |

## Support the project

[Support the project](https://buymeacoffee.com/tylernol)
