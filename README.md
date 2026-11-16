# Shaiya Toolkit

Command-line and desktop tools for Shaiya game data, built on one tested Rust core: read and describe client files today; browse archives, build patches and edit SData tables as the tools below land.

> **These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. A tool never changes a file in place without making a backup first or writing to a path you name.**

## Tools

| Tool | What it does | CLI command | GUI program | Status |
|---|---|---|---|---|
| File inspector | Identify a Shaiya file (SAH index, SAF data, SData, patch, game.exe) and show what it is; SHA-256 | `shaiya info` | Shaiya Workbench (`shaiya-workbench`) | available |
| Archive & patches | Browse, extract and edit `data.sah`/`data.saf`; build, check and apply `psNNNN.patch` | `shaiya archive`, `shaiya patch` | Shaiya Workbench | planned |
| SData schemas and editor | Read and edit the client's SData tables for each episode | `shaiya sdata` | Shaiya SData Editor | planned |
| Packet log decoder | Decode game packet logs | `shaiya log` | Shaiya Log Viewer | planned |
| Server map editor | Edit server map files | `shaiya svmap` | Shaiya SVMap Editor | planned |

## Download

No release has been published yet; the first one comes with the archive and patch tools. Releases will carry these files (see [docs/install.md](docs/install.md) for how to check them):

| Program | Windows | Linux | macOS |
|---|---|---|---|
| Everything | `shaiya-toolkit-<version>-windows-x86_64.zip` | `shaiya-toolkit-<version>-linux-x86_64.zip` | `shaiya-toolkit-<version>-macos-universal.zip` |
| `shaiya` (command line) | `shaiya-<version>-windows-x86_64.zip` | `shaiya-<version>-linux-x86_64.zip` | `shaiya-<version>-macos-universal.zip` |
| Shaiya Workbench | `shaiya-workbench-<version>-windows-x86_64.zip` | `shaiya-workbench-<version>-linux-x86_64.zip` | `shaiya-workbench-<version>-macos-universal.zip` |

All downloads: the [releases page](https://github.com/byyako/shaiya-toolkit/releases). Every release has a `SHA256SUMS.txt`.

Platform notes:

- **Windows:** the programs are not code signed, so SmartScreen may warn the first time. Choose More info, then Run anyway.
- **macOS:** the apps are not notarised. If macOS says the developer is unidentified, right-click the app and choose Open, or run `xattr -dr com.apple.quarantine <app>`. The macOS GUI is **untested** until a tester confirms it.
- **Linux:** the GUI programs need a desktop with X11 or Wayland and a Vulkan or OpenGL driver.

## Which clients work

The toolkit reads the client archive and SData wrapper of every client the project could test (US, RU, FR, CN, Phoenix, KR episode 8, Fawkes v68); what parses on which client is in [docs/version-matrix.md](docs/version-matrix.md).

## Build from source

With stock Rust (`rustup`, stable):

```
cargo build --release
```

The programs are `target/release/shaiya` and `target/release/shaiya-workbench`. See [CONTRIBUTING.md](CONTRIBUTING.md) for tests and the development workflow, and [docs/](docs/README.md) for formats and architecture.

## Licence

MIT, see [LICENSE](LICENSE). Made by byyako. Third-party components: [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).
