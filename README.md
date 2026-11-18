# Shaiya Toolkit

The Shaiya Toolkit repository holds command-line and desktop tools for Shaiya game data, built on one tested Rust core. **Shaiya Toolkit** is the desktop app (`shaiya-toolkit`); **`shaiya`** is the command line. With them you can browse, extract and edit a client's `data.sah`/`data.saf`, build, sign, apply and roll back `psNNNN.patch` files, convert SData files, and identify any client file. Changes to a client folder are journaled so they can usually be rolled back (the note below says what cannot). The SData table editor and the other tools below are planned.

> **These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. Changes to a client folder are recorded in a journal so they can usually be rolled back, but not changes made with --no-journal or archive compact --in-place, which keep at most the previous index (data.sah.bak). Everything else is written only to a path you name.**

## Tools

| Tool | What it does | CLI command | GUI program | Status |
|---|---|---|---|---|
| File inspector | Identify a Shaiya file (SAH index, SAF data, SData, patch, game.exe) and show what it is; SHA-256 | `shaiya info` | Shaiya Toolkit (`shaiya-toolkit`) | available |
| Archive & patches | Browse, extract and edit `data.sah`/`data.saf` with a journal you can roll back; build, sign, verify, apply and roll back `psNNNN.patch`; compare, compact, pack and swap archives | `shaiya archive`, `shaiya patch`, `shaiya clients` | Shaiya Toolkit | available |
| SData container | Describe, check, decrypt and encrypt SData files (the SEED wrapper), byte-identical round trips | `shaiya sdata` | Shaiya Toolkit (SData tab) | available |
| SData schemas and editor | Read and edit the client's SData tables for each episode | (planned) | Shaiya SData Editor | planned |
| Packet log decoder | Decode game packet logs | `shaiya log` | Shaiya Log Viewer | planned |
| Server map editor | Edit server map files | `shaiya svmap` | Shaiya SVMap Editor | planned |

## Download

Each release carries these files (see [docs/install.md](docs/install.md) for how to check them):

| Download | Windows | Linux | macOS |
|---|---|---|---|
| Shaiya Toolkit (desktop app, includes `shaiya`) | `shaiya-toolkit-<version>-windows-x86_64.zip` | `shaiya-toolkit-<version>-linux-x86_64.zip` | `shaiya-toolkit-<version>-macos-universal.zip` |
| `shaiya` (command line) | `shaiya-<version>-windows-x86_64.zip` | `shaiya-<version>-linux-x86_64.zip` | `shaiya-<version>-macos-universal.zip` |

All downloads: the [releases page](https://github.com/byyako/shaiya-toolkit/releases). Every release has a `SHA256SUMS.txt`.

Platform notes:

- **Windows:** the programs are not code signed, so SmartScreen may warn the first time. Choose More info, then Run anyway.
- **macOS:** the apps are not notarised. If macOS says the developer is unidentified, right-click the app and choose Open, or run `xattr -dr com.apple.quarantine <app>`. The macOS GUI is **untested** until a tester confirms it.
- **Linux:** the GUI programs need a desktop with X11 or Wayland and a Vulkan or OpenGL driver.

## Getting started

[Getting started in five minutes](docs/getting-started.md) takes you from inspecting a file to building and applying a patch on a copy and undoing it. Guides: [modding files](docs/guides/mod-files.md), [making patches](docs/guides/make-a-patch.md), [SData files](docs/guides/sdata-files.md), [undo and recovery](docs/guides/undo-and-recovery.md); the [FAQ](docs/faq.md) covers the common errors.

## Which clients work

The toolkit reads, edits and patches the client archive, and reads the SData wrapper, of every client the project could test (US, RU, FR, CN, Phoenix, KR episode 8, Fawkes v68); what works on which client is in [docs/version-matrix.md](docs/version-matrix.md).

## Build from source

With stock Rust (`rustup`, stable):

```
cargo build --release
```

The programs are `target/release/shaiya` and `target/release/shaiya-toolkit`. On Linux the GUI needs a few development packages, listed in [docs/install.md](docs/install.md#build-from-source). See [CONTRIBUTING.md](CONTRIBUTING.md) for tests and the development workflow, and [docs/](docs/README.md) for formats and architecture.

## Licence

MIT, see [LICENSE](LICENSE). Made by byyako. Third-party components: [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).

[Support the project](https://buymeacoffee.com/tylernol)
