# Shaiya Toolkit

The Shaiya Toolkit repository holds command-line and desktop tools for Shaiya game data, built on one tested Rust core. **Shaiya Toolkit** is the desktop app (`shaiya-toolkit`); **`shaiya`** is the command line. **Shaiya SVMap Editor** (`shaiya-svmap-editor`) is the desktop editor for server map files. With these programs you can browse, extract and edit a client's `data.sah`/`data.saf`, build, sign, apply and roll back `psNNNN.patch` files, convert SData files, identify any client file, and edit, check and make server maps (`.svmap`) with their client files. Changes the toolkit's other tools make to a client folder are journaled so they can usually be rolled back (the note below says what cannot). The SData table editor and the other tools marked planned below are not built yet.

> **These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. Changes the toolkit's other tools make to a client folder are recorded in a journal so they can usually be rolled back, but not changes made with --no-journal or archive compact --in-place, which keep at most the previous index (data.sah.bak). Shaiya SVMap Editor saves server map files, Map.ini and MapWar.ini where you open them (keeping a .bak of the first version), writes client files to a staging folder you pick, writes patches and folder exports only where you say, and changes data.sah/data.saf only of a test client you pick with Install, which keeps its own copy of the index there (no journal) so Uninstall can restore it; Copy loose adds new files to that test client. It never writes the client it reads from, and never edits game.exe or a DLL. Everything else is written only to a path you name, apart from temporary files in the system temp folder.**

## Tools

| Tool | What it does | CLI command | GUI program | Status |
|---|---|---|---|---|
| File inspector | Identify a Shaiya file (SAH index, SAF data, SData, patch, game.exe) and show what it is; SHA-256 | `shaiya info` | Shaiya Toolkit (`shaiya-toolkit`) | available |
| Archive & patches | Browse, extract and edit `data.sah`/`data.saf` with a journal you can roll back; build, sign, verify, apply and roll back `psNNNN.patch`, publish them for the original updater (`UpdateVersion.ini`); compare, compact, pack and swap archives | `shaiya archive`, `shaiya patch`, `shaiya clients` | Shaiya Toolkit | available |
| SData container | Describe, check, decrypt and encrypt SData files (the SEED wrapper), byte-identical round trips | `shaiya sdata` | Shaiya Toolkit (SData tab) | available |
| SData tables (view) | Show Item, Skill, Monster and NpcQuest files as tables, export CSV or JSON; read only, one layout per format variant, a table says whether its layout was read from a client's own loader (loader-verified) or only fits the file (FIT-ONLY), a file no layout fits is shown as hex | `shaiya sdata table` | Shaiya Toolkit (SData tab, Archive preview) | available |
| Map, model and animation preview | Preview `world/<id>.wld` maps as a top view and `.smod` models as a textured, lit picture in the Archive tab; play `.ANI` animations on a mesh found by name and preview `.3DC` meshes; describe and check them and compare a replacement with the file it replaces | `shaiya anim` (the previews: Archive tab) | Shaiya Toolkit (Archive preview) | available |
| Client health and clean-up | Look at a client folder (index check, dead space, journals, unfinished change, leftovers) and move leftovers into a cleanup folder, or put them back; none of them is deleted, and an interrupted change is finished or discarded first by recovery | `shaiya clients health`, `shaiya clients clean` | Shaiya Toolkit (File > Client health...) | available |
| Several test clients | A queue of pending changes for each client, Compare any two saved clients or archive files and make a patch from the differences, apply patches to several test clients one after the other (own confirmation and journal for each) | (Toolkit only) | Shaiya Toolkit (Archive and Apply & history tabs) | available |
| SData editor | Edit the client's SData tables for each episode | (planned) | Shaiya SData Editor | planned |
| Packet log decoder | Decode game packet logs | `shaiya log` | Shaiya Log Viewer | planned |
| Server map editor | Edit, check and make server map files (`.svmap`), their client `.wld` copies, `Map.ini`; export a patch; check maps from the command line | `shaiya svmap` | Shaiya SVMap Editor (`shaiya-svmap-editor`) | available |

## Download

Each release carries these files (see [docs/install.md](docs/install.md) for how to check them):

| Download | Windows | Linux | macOS |
|---|---|---|---|
| Shaiya Toolkit (desktop apps Shaiya Toolkit and Shaiya SVMap Editor, includes `shaiya`) | `shaiya-toolkit-<version>-windows-x86_64.zip` | `shaiya-toolkit-<version>-linux-x86_64.zip` | `shaiya-toolkit-<version>-macos-universal.zip` |
| `shaiya` (command line) | `shaiya-<version>-windows-x86_64.zip` | `shaiya-<version>-linux-x86_64.zip` | `shaiya-<version>-macos-universal.zip` |

All downloads: the [releases page](https://github.com/byyako/shaiya-toolkit/releases). Every release has a `SHA256SUMS.txt`.

Platform notes:

- **Windows:** the programs are not code signed, so SmartScreen may warn the first time. Choose More info, then Run anyway.
- **macOS:** the apps are not notarised. If macOS says the developer is unidentified, right-click the app and choose Open, or run `xattr -dr com.apple.quarantine <app>`. The macOS GUI is **untested** until a tester confirms it, and so is the Linux build of Shaiya SVMap Editor.
- **Linux:** the GUI programs need a desktop with X11 or Wayland and a Vulkan or OpenGL driver.

## Getting started

[Getting started in five minutes](docs/getting-started.md) takes you from inspecting a file to building and applying a patch on a copy and undoing it. Guides: [modding files](docs/guides/mod-files.md), [making patches](docs/guides/make-a-patch.md), [SData files](docs/guides/sdata-files.md), [replacing an animation](docs/guides/replacing-an-animation.md), [undo and recovery](docs/guides/undo-and-recovery.md); the [FAQ](docs/faq.md) covers the common errors.

## Which clients work

The toolkit reads, edits and patches the client archive, and reads the SData wrapper, of every client the project could test (US, RU, FR, CN, Phoenix, KR episode 8, Fawkes v68); what works on which client is in [docs/version-matrix.md](docs/version-matrix.md). The SData table view reads a file only when a layout consumes it to the last byte; the layouts were checked on the US, RU, FR and Phoenix clients and the page says which variant read which client. Shaiya SVMap Editor and `shaiya svmap` were written for and verified on the US EP4.5 client (patch 108) and the original EP4.5 and Afterglow server files. Other episodes are detected and the checks that depend on EP4.5 data say so; the same page lists the assumptions. Patches the editor exports are unsigned; sign them with `shaiya patch sign` or Shaiya Toolkit if your updater needs a signature.

## Build from source

With stock Rust (`rustup`, stable):

```
cargo build --release
```

The programs are `target/release/shaiya`, `target/release/shaiya-toolkit` and `target/release/shaiya-svmap-editor`. On Linux the GUI needs a few development packages, listed in [docs/install.md](docs/install.md#build-from-source). See [CONTRIBUTING.md](CONTRIBUTING.md) for tests and the development workflow, and [docs/](docs/README.md) for formats and architecture.

## Licence

MIT, see [LICENSE](LICENSE). Made by byyako. Third-party components: [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).

[Support the project](https://buymeacoffee.com/tylernol)
